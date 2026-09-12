//! Display admission on the exact hash-verified handle; never rewrites source bytes.

use super::super::storage_io_error;
use lorepia_domain::{AssetDescriptor, CoreError, CoreErrorCode, CoreResult};
use std::{
    fs::File,
    io::{self, BufReader, Read, Seek, SeekFrom},
};

mod png;
const MAX_IMAGE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SIDE: usize = 8192;
const MAX_PIXELS: usize = 16_777_216;

pub(super) fn validate(
    file: &mut File,
    descriptor: &AssetDescriptor,
) -> CoreResult<Option<Vec<u8>>> {
    if !descriptor.media_type.starts_with("image/") {
        return Ok(None);
    }
    // imagesize's HEIF area comparison uses usize multiplication internally.
    // Reject AVIF before entering that parser on narrower native targets.
    #[cfg(target_pointer_width = "32")]
    if descriptor.media_type == "image/avif" {
        return Err(CoreError::new(
            CoreErrorCode::UnsupportedContent,
            "AVIF dimension validation requires a 64-bit native target",
            false,
        ));
    }
    // Bound repeated reads and seeks as well as compressed length. The parser
    // does not decode pixels and cannot seek outside this same verified file.
    let mut reader = BoundedReader {
        file: &mut *file,
        length: descriptor.size_bytes,
        remaining_bytes: 2 * MAX_IMAGE_BYTES,
        remaining_ops: 8192,
        failed: false,
    };
    let dimensions = imagesize::reader_size(BufReader::new(&mut reader)).map_err(|_| {
        CoreError::new(
            CoreErrorCode::UnsupportedContent,
            "image dimensions could not be established within display bounds",
            false,
        )
    })?;
    if reader.failed {
        return Err(CoreError::new(
            CoreErrorCode::UnsupportedContent,
            "image header parsing exceeded bounded work",
            false,
        ));
    }
    if dimensions.width == 0
        || dimensions.height == 0
        || dimensions.width > MAX_SIDE
        || dimensions.height > MAX_SIDE
        || dimensions
            .width
            .checked_mul(dimensions.height)
            .is_none_or(|pixels| pixels > MAX_PIXELS)
    {
        return Err(CoreError::new(
            CoreErrorCode::UnsupportedContent,
            "image dimensions exceed the supported single-frame display budget",
            false,
        ));
    }
    if descriptor.media_type == "image/png" {
        file.rewind().map_err(storage_io_error)?;
        let mut bytes = vec![
            0;
            usize::try_from(descriptor.size_bytes).map_err(|_| {
                CoreError::new(
                    CoreErrorCode::UnsupportedContent,
                    "image is too large",
                    false,
                )
            })?
        ];
        file.read_exact(&mut bytes).map_err(storage_io_error)?;
        if !png::has_valid_chunk_checksums(&bytes) {
            return Err(CoreError::new(
                CoreErrorCode::UnsupportedContent,
                "image PNG chunk checksums are invalid",
                false,
            ));
        }
        return Ok(Some(bytes));
    }
    Ok(None)
}

struct BoundedReader<'a> {
    file: &'a mut File,
    length: u64,
    remaining_bytes: u64,
    remaining_ops: usize,
    failed: bool,
}

impl BoundedReader<'_> {
    fn charge(&mut self) -> io::Result<()> {
        if self.remaining_ops == 0 {
            self.failed = true;
        }
        self.remaining_ops = self
            .remaining_ops
            .checked_sub(1)
            .ok_or_else(|| io::Error::other("image header work budget exhausted"))?;
        Ok(())
    }
}

impl Read for BoundedReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.charge()?;
        if self.remaining_bytes == 0 {
            self.failed = true;
            return Err(io::Error::other("image header read budget exhausted"));
        }
        let limit = buffer
            .len()
            .min(usize::try_from(self.remaining_bytes).unwrap_or(usize::MAX));
        let count = self.file.read(&mut buffer[..limit])?;
        self.remaining_bytes -= count as u64;
        Ok(count)
    }
}

impl Seek for BoundedReader<'_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.charge()?;
        let offset = match position {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::End(delta) => self.length.checked_add_signed(delta),
            SeekFrom::Current(delta) => self.file.stream_position()?.checked_add_signed(delta),
        }
        .filter(|offset| *offset <= self.length)
        .ok_or_else(|| {
            self.failed = true;
            io::Error::other("image header seek exceeds file bounds")
        })?;
        self.file.seek(SeekFrom::Start(offset))
    }
}

#[cfg(test)]
mod tests;
