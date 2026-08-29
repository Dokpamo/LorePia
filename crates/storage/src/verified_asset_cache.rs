//! Short-lived, bounded leases for already verified CAS asset handles.

use std::{
    collections::VecDeque,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
    time::{Duration, Instant, SystemTime},
};

use lorepia_domain::AssetDescriptor;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

const DEFAULT_MAX_HANDLES: usize = 16;
const DEFAULT_LEASE_TTL: Duration = Duration::from_secs(30);
const DEFAULT_MAX_VERIFICATIONS_PER_WINDOW: usize = 32;
const DEFAULT_VERIFICATION_WINDOW: Duration = Duration::from_mins(1);

pub(crate) enum CacheLookup<T> {
    Hit(T),
    Miss,
    Changed,
}

pub(crate) struct VerifiedAssetCache {
    entries: VecDeque<VerifiedAssetHandle>,
    verification_started: VecDeque<Instant>,
    max_handles: usize,
    lease_ttl: Duration,
    max_verifications_per_window: usize,
    verification_window: Duration,
}

struct VerifiedAssetHandle {
    descriptor: AssetDescriptor,
    file: AssetFileSnapshot,
    verified_at: Instant,
}

pub(crate) struct AssetFileSnapshot {
    file: File,
    identity: FileIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    links: u64,
    #[cfg(unix)]
    modified_seconds: i64,
    #[cfg(unix)]
    modified_nanoseconds: i64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanoseconds: i64,
    #[cfg(windows)]
    volume_serial_number: u32,
    #[cfg(windows)]
    file_index: u64,
    #[cfg(windows)]
    links: u32,
    #[cfg(windows)]
    last_write_time: u64,
}

impl FileIdentity {
    fn read(file: &File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        #[cfg(windows)]
        let windows = windows_file_information(file)?;

        Ok(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            links: metadata.nlink(),
            #[cfg(unix)]
            modified_seconds: metadata.mtime(),
            #[cfg(unix)]
            modified_nanoseconds: metadata.mtime_nsec(),
            #[cfg(unix)]
            changed_seconds: metadata.ctime(),
            #[cfg(unix)]
            changed_nanoseconds: metadata.ctime_nsec(),
            #[cfg(windows)]
            volume_serial_number: windows.dwVolumeSerialNumber,
            #[cfg(windows)]
            file_index: u64::from(windows.nFileIndexHigh) << 32 | u64::from(windows.nFileIndexLow),
            #[cfg(windows)]
            links: windows.nNumberOfLinks,
            #[cfg(windows)]
            last_write_time: u64::from(windows.ftLastWriteTime.dwHighDateTime) << 32
                | u64::from(windows.ftLastWriteTime.dwLowDateTime),
        })
    }
}

impl AssetFileSnapshot {
    pub(crate) fn capture(file: File) -> io::Result<Self> {
        let identity = FileIdentity::read(&file)?;
        Ok(Self { file, identity })
    }

    pub(crate) fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    pub(crate) fn ensure_unchanged(&self) -> io::Result<()> {
        if FileIdentity::read(&self.file)? == self.identity {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "asset identity changed during verification",
            ))
        }
    }
}

impl VerifiedAssetCache {
    pub(crate) fn new(max_handles: usize, lease_ttl: Duration) -> Self {
        Self::with_limits(
            max_handles,
            lease_ttl,
            DEFAULT_MAX_VERIFICATIONS_PER_WINDOW,
            DEFAULT_VERIFICATION_WINDOW,
        )
    }

    fn with_limits(
        max_handles: usize,
        lease_ttl: Duration,
        max_verifications_per_window: usize,
        verification_window: Duration,
    ) -> Self {
        Self {
            entries: VecDeque::new(),
            verification_started: VecDeque::new(),
            max_handles: max_handles.max(1),
            lease_ttl,
            max_verifications_per_window: max_verifications_per_window.max(1),
            verification_window,
        }
    }

    pub(crate) fn begin_verification(&mut self) -> io::Result<()> {
        let now = Instant::now();
        self.verification_started
            .retain(|started| now.duration_since(*started) < self.verification_window);
        if self.verification_started.len() >= self.max_verifications_per_window {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "verified asset hash budget is temporarily exhausted",
            ));
        }
        self.verification_started.push_back(now);
        Ok(())
    }

    pub(crate) fn contains_verified(
        &mut self,
        descriptor: &AssetDescriptor,
    ) -> io::Result<CacheLookup<()>> {
        self.with_entry(descriptor, |_| Ok(()))
    }

    pub(crate) fn read_range(
        &mut self,
        descriptor: &AssetDescriptor,
        start: u64,
        length: u64,
    ) -> io::Result<CacheLookup<Vec<u8>>> {
        self.with_entry(descriptor, |entry| {
            let capacity = usize::try_from(length)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "range is too large"))?;
            let mut bytes = vec![0_u8; capacity];
            entry.seek(SeekFrom::Start(start))?;
            entry.read_exact(&mut bytes)?;
            Ok(bytes)
        })
    }

    pub(crate) fn insert(
        &mut self,
        descriptor: AssetDescriptor,
        file: AssetFileSnapshot,
    ) -> io::Result<()> {
        file.ensure_unchanged()?;
        self.entries
            .retain(|entry| entry.descriptor.sha256 != descriptor.sha256);
        while self.entries.len() >= self.max_handles {
            self.entries.pop_front();
        }
        self.entries.push_back(VerifiedAssetHandle {
            descriptor,
            file,
            verified_at: Instant::now(),
        });
        Ok(())
    }

    fn with_entry<T>(
        &mut self,
        descriptor: &AssetDescriptor,
        operation: impl FnOnce(&mut File) -> io::Result<T>,
    ) -> io::Result<CacheLookup<T>> {
        let now = Instant::now();
        self.entries
            .retain(|entry| now.duration_since(entry.verified_at) < self.lease_ttl);
        let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.descriptor.sha256 == descriptor.sha256)
        else {
            return Ok(CacheLookup::Miss);
        };
        let Some(mut entry) = self.entries.remove(index) else {
            return Ok(CacheLookup::Miss);
        };
        if !same_file_contract(&entry.descriptor, descriptor)
            || entry.file.ensure_unchanged().is_err()
        {
            return Ok(CacheLookup::Changed);
        }
        let value = operation(entry.file.file_mut())?;
        if entry.file.ensure_unchanged().is_err() {
            return Ok(CacheLookup::Changed);
        }
        self.entries.push_back(entry);
        Ok(CacheLookup::Hit(value))
    }
}

fn same_file_contract(left: &AssetDescriptor, right: &AssetDescriptor) -> bool {
    left.sha256 == right.sha256
        && left.size_bytes == right.size_bytes
        && left.media_type == right.media_type
}

impl Default for VerifiedAssetCache {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_HANDLES, DEFAULT_LEASE_TTL)
    }
}

/// Opens the exact digest file without following a final symlink or reparse point.
#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
pub(crate) fn open_asset_file(root: &Path, sha256: &str) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, open, openat};

    let directory_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    let file_flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    let assets = open(root.join("assets"), directory_flags, Mode::empty())?;
    let sha256_dir = openat(&assets, "sha256", directory_flags, Mode::empty())?;
    let prefix = sha256
        .get(..2)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid asset digest"))?;
    let basename = sha256
        .get(2..)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid asset digest"))?;
    let prefix_dir = openat(&sha256_dir, prefix, directory_flags, Mode::empty())?;
    let file = openat(&prefix_dir, basename, file_flags, Mode::empty())?;
    Ok(File::from(file))
}

#[cfg(windows)]
pub(crate) fn open_asset_file(root: &Path, sha256: &str) -> io::Result<File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
    };

    let prefix = sha256
        .get(..2)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid asset digest"))?;
    let basename = sha256
        .get(2..)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid asset digest"))?;
    let path = root
        .join("assets")
        .join("sha256")
        .join(prefix)
        .join(basename);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "asset is a reparse point",
        ));
    }
    if windows_file_information(&file)?.nNumberOfLinks != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "asset has hard-link aliases",
        ));
    }
    Ok(file)
}

#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "GetFileInformationByHandle is required for stable file identity and link count"
)]
fn windows_file_information(
    file: &File,
) -> io::Result<windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION> {
    use std::{mem::MaybeUninit, os::windows::io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: `file` owns a valid handle for the duration of the call and the
    // API initializes the complete output structure when it reports success.
    let succeeded =
        unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
    if succeeded == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful API call above initialized the output structure.
    Ok(unsafe { information.assume_init() })
}

#[cfg(not(any(
    target_os = "android",
    target_os = "linux",
    target_vendor = "apple",
    windows
)))]
pub(crate) fn open_asset_file(root: &Path, sha256: &str) -> io::Result<File> {
    let prefix = sha256
        .get(..2)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid asset digest"))?;
    let basename = sha256
        .get(2..)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid asset digest"))?;
    std::fs::OpenOptions::new()
        .read(true)
        .open(root.join("assets/sha256").join(prefix).join(basename))
}

#[cfg(test)]
mod tests {
    use std::{io::Write, thread};

    use lorepia_domain::{AssetId, AssetRole, AssetSource, AssetSourceKind, Sha256Digest};
    use tempfile::NamedTempFile;

    use super::*;

    fn descriptor(sha256: &str, size_bytes: u64) -> AssetDescriptor {
        AssetDescriptor {
            id: AssetId::from("asset"),
            sha256: Sha256Digest::parse(sha256).expect("digest"),
            media_type: "video/mp4".to_owned(),
            role: AssetRole::Attachment,
            name: "asset.mp4".to_owned(),
            size_bytes,
            width: None,
            height: None,
            duration_ms: Some(1),
            source: AssetSource {
                kind: AssetSourceKind::CharxPackage,
                source_sha256: None,
                logical_path: Some("assets/asset.mp4".to_owned()),
            },
        }
    }

    #[test]
    fn repeated_ranges_reuse_one_verified_handle_and_detect_mutation() {
        let mut source = NamedTempFile::new().expect("temp file");
        source.write_all(b"01234567").expect("write");
        source.flush().expect("flush");
        let digest = "ab".repeat(32);
        let descriptor = descriptor(&digest, 8);
        let file = source.reopen().expect("reopen");
        let mut cache = VerifiedAssetCache::new(2, Duration::from_secs(30));
        cache
            .insert(
                descriptor.clone(),
                AssetFileSnapshot::capture(file).expect("snapshot"),
            )
            .expect("insert");

        let CacheLookup::Hit(first) = cache.read_range(&descriptor, 1, 3).expect("first range")
        else {
            panic!("expected cache hit");
        };
        let CacheLookup::Hit(second) = cache.read_range(&descriptor, 4, 2).expect("second range")
        else {
            panic!("expected cache hit");
        };
        assert_eq!(first, b"123");
        assert_eq!(second, b"45");

        source.as_file_mut().set_len(4).expect("mutate");
        let result = cache
            .read_range(&descriptor, 0, 1)
            .expect("mutation result");
        assert!(matches!(result, CacheLookup::Changed));
    }

    #[test]
    fn lease_expiry_forces_reverification() {
        let mut source = NamedTempFile::new().expect("temp file");
        source.write_all(b"data").expect("write");
        source.flush().expect("flush");
        let digest = "cd".repeat(32);
        let descriptor = descriptor(&digest, 4);
        let mut cache = VerifiedAssetCache::new(1, Duration::from_millis(1));
        cache
            .insert(
                descriptor.clone(),
                AssetFileSnapshot::capture(source.reopen().expect("reopen")).expect("snapshot"),
            )
            .expect("insert");
        thread::sleep(Duration::from_millis(3));
        assert!(matches!(
            cache.contains_verified(&descriptor).expect("lookup"),
            CacheLookup::Miss
        ));
    }

    #[test]
    fn insertion_rejects_identity_changes_after_verification_started() {
        let mut source = NamedTempFile::new().expect("temp file");
        source.write_all(b"01234567").expect("write");
        source.flush().expect("flush");
        let digest = "ef".repeat(32);
        let descriptor = descriptor(&digest, 8);
        let snapshot =
            AssetFileSnapshot::capture(source.reopen().expect("reopen")).expect("snapshot");

        source
            .as_file_mut()
            .set_len(4)
            .expect("truncate after snapshot");

        let mut cache = VerifiedAssetCache::new(1, Duration::from_secs(30));
        assert!(cache.insert(descriptor, snapshot).is_err());
    }

    #[test]
    fn verification_budget_prevents_lru_hash_thrash() {
        let mut cache =
            VerifiedAssetCache::with_limits(1, Duration::from_secs(30), 2, Duration::from_mins(1));
        cache.begin_verification().expect("first verification");
        cache.begin_verification().expect("second verification");
        let error = cache
            .begin_verification()
            .expect_err("third verification must be rate-limited");
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    }
}
