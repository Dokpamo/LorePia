use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    time::Instant,
};

use async_trait::async_trait;
use lorepia_chat::run_generation;
use lorepia_content::{inspect_file, sha256_file};
use lorepia_core::{Character, ChatEventKind, Conversation, Core, CoreConfig, Message};
use lorepia_domain::{
    ConversationId, CoreError, CoreErrorCode, CoreResult, GenerationId, GenerationRequest,
    GenerationUsage, ImportLimits, ProviderCapabilities,
};
use lorepia_providers::{Provider, ProviderEvent, ProviderEventSender, StaticProvider};
use lorepia_storage::Storage;
use tempfile::{NamedTempFile, TempDir, tempdir};
use tokio::sync::{mpsc, watch};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

const LARGE_ASSET_BYTES: usize = 32 * 1024 * 1024;
const ASSET_CATALOG_SIZE: usize = 2_000;
const LONG_STREAM_CHUNKS: usize = 4_096;
const LONG_STREAM_CHUNK: &str = "abcdefgh";
const RAPID_REGENERATION_CYCLES: usize = 100;
const RECOVERY_MESSAGE_ROWS: usize = 1_000;
const CARD_JSON: &[u8] =
    br#"{"spec":"chara_card_v3","data":{"name":"Measurement","description":"Synthetic"}}"#;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

struct SyntheticPackage {
    _directory: TempDir,
    path: PathBuf,
}

impl SyntheticPackage {
    fn path(&self) -> &Path {
        &self.path
    }
}

fn synthetic_card() -> NamedTempFile {
    let mut card = tempfile::Builder::new()
        .suffix(".json")
        .tempfile()
        .expect("temporary card");
    card.write_all(CARD_JSON).expect("write card");
    card
}

fn stored_zip_options() -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o644)
}

fn large_synthetic_package() -> SyntheticPackage {
    let directory = tempdir().expect("temporary package directory");
    let path = directory.path().join("large.charx");
    let file = File::create(&path).expect("create large package");
    let mut archive = ZipWriter::new(file);
    let options = stored_zip_options();
    archive
        .start_file("card.json", options)
        .expect("start card metadata");
    archive.write_all(CARD_JSON).expect("write card metadata");
    archive
        .start_file("assets/large.png", options)
        .expect("start large asset");
    archive
        .write_all(PNG_SIGNATURE)
        .expect("write asset signature");
    let chunk = [0xa5_u8; 8 * 1024];
    let mut remaining = LARGE_ASSET_BYTES - PNG_SIGNATURE.len();
    while remaining > 0 {
        let write_len = remaining.min(chunk.len());
        archive
            .write_all(&chunk[..write_len])
            .expect("write large asset chunk");
        remaining -= write_len;
    }
    archive.finish().expect("finish large package");
    SyntheticPackage {
        _directory: directory,
        path,
    }
}

fn asset_catalog_package() -> SyntheticPackage {
    let directory = tempdir().expect("temporary package directory");
    let path = directory.path().join("asset-catalog.charx");
    let file = File::create(&path).expect("create asset catalog");
    let mut archive = ZipWriter::new(file);
    let options = stored_zip_options();
    archive
        .start_file("card.json", options)
        .expect("start card metadata");
    archive.write_all(CARD_JSON).expect("write card metadata");
    for index in 0..ASSET_CATALOG_SIZE {
        archive
            .start_file(format!("assets/{index:04}.png"), options)
            .expect("start synthetic asset");
        archive
            .write_all(PNG_SIGNATURE)
            .expect("write synthetic asset");
    }
    archive.finish().expect("finish asset catalog");
    SyntheticPackage {
        _directory: directory,
        path,
    }
}

fn synthetic_generation_request() -> GenerationRequest {
    GenerationRequest {
        generation_id: GenerationId::new(),
        conversation_id: ConversationId::new(),
        model: "synthetic-performance-provider".to_owned(),
        messages: Vec::new(),
        temperature: Some(0.0),
        max_output_tokens: Some(4_096),
        resolved_prompt_plan: None,
        provider_execution_plan_hash: None,
        provider_provenance: None,
        preserve_opaque_reasoning_state: false,
        opaque_reasoning_context: Vec::new(),
    }
}

fn cancelled_error() -> CoreError {
    CoreError::new(
        CoreErrorCode::Cancelled,
        "synthetic generation was cancelled",
        true,
    )
}

struct ChunkedProvider {
    chunks: usize,
    chunk: &'static str,
}

#[async_trait]
impl Provider for ChunkedProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            reasoning: false,
            max_context_tokens: None,
        }
    }

    async fn generate(
        &self,
        _request: GenerationRequest,
        _credential: Option<&str>,
        sink: ProviderEventSender,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationUsage> {
        for _ in 0..self.chunks {
            if *cancelled.borrow() {
                return Err(cancelled_error());
            }
            sink.send(ProviderEvent::TextDelta(self.chunk.to_owned()))
                .await
                .map_err(|_| CoreError::internal("performance event receiver closed"))?;
        }
        Ok(GenerationUsage::default())
    }
}

struct CancelAwareProvider;

#[async_trait]
impl Provider for CancelAwareProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            reasoning: false,
            max_context_tokens: None,
        }
    }

    async fn generate(
        &self,
        _request: GenerationRequest,
        _credential: Option<&str>,
        sink: ProviderEventSender,
        mut cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationUsage> {
        sink.send(ProviderEvent::TextDelta("partial".to_owned()))
            .await
            .map_err(|_| CoreError::internal("performance event receiver closed"))?;
        loop {
            if *cancelled.borrow() {
                return Err(cancelled_error());
            }
            cancelled
                .changed()
                .await
                .map_err(|_| CoreError::internal("performance cancellation sender closed"))?;
        }
    }
}

#[test]
#[ignore = "manual performance measurement; no pass/fail duration budget"]
fn opens_a_library_with_one_thousand_characters() {
    let root = tempdir().expect("temporary data root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let card = synthetic_card();

    let write_started = Instant::now();
    for _ in 0..1_000 {
        let inspection = core.inspect_import(card.path()).expect("inspect");
        core.commit_import(&inspection.id).expect("commit");
    }
    let write_elapsed = write_started.elapsed();
    drop(core);

    let open_started = Instant::now();
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen core");
    let open_elapsed = open_started.elapsed();
    let read_started = Instant::now();
    let characters = reopened.list_characters().expect("list characters");
    let read_elapsed = read_started.elapsed();
    assert_eq!(characters.len(), 1_000);
    eprintln!(
        "scenario=library_1000 write_ms={} open_ms={} list_ms={}",
        write_elapsed.as_millis(),
        open_elapsed.as_millis(),
        read_elapsed.as_millis()
    );
}

#[test]
#[ignore = "manual performance measurement; writes 100,000 synthetic rows"]
fn loads_one_hundred_thousand_message_metadata_rows() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    let card = synthetic_card();
    let source_hash = sha256_file(card.path()).expect("source hash");
    let character = Character::new("Measurement", "Synthetic", source_hash);
    storage
        .commit_character_import(
            card.path(),
            &character,
            card.as_file().metadata().expect("card metadata").len(),
            "performance-import",
            &[],
        )
        .expect("commit character");
    let conversation = Conversation::new(&character.id, "Measurement");
    storage
        .save_conversation(&conversation)
        .expect("save conversation");

    let write_started = Instant::now();
    for index in 0..100_000 {
        storage
            .save_message(&Message::user(
                conversation.id.clone(),
                format!("synthetic message {index}"),
            ))
            .expect("save message");
    }
    let write_elapsed = write_started.elapsed();

    let read_started = Instant::now();
    let messages = storage
        .list_messages(&conversation.id)
        .expect("list messages");
    let read_elapsed = read_started.elapsed();
    assert_eq!(messages.len(), 100_000);
    eprintln!(
        "scenario=messages_100000 write_ms={} read_ms={}",
        write_elapsed.as_millis(),
        read_elapsed.as_millis()
    );
}

#[test]
#[ignore = "manual performance measurement; creates a 32 MiB synthetic package"]
fn inspects_a_large_package() {
    let package = large_synthetic_package();
    let package_bytes = fs::metadata(package.path())
        .expect("large package metadata")
        .len();

    let inspect_started = Instant::now();
    let inspection =
        inspect_file(package.path(), ImportLimits::default()).expect("inspect large package");
    let inspect_elapsed = inspect_started.elapsed();

    assert_eq!(inspection.asset_count, 1);
    assert_eq!(
        inspection.estimated_stored_size,
        u64::try_from(LARGE_ASSET_BYTES + CARD_JSON.len()).expect("package size fits in u64")
    );
    eprintln!(
        "scenario=large_package_inspect package_bytes={package_bytes} uncompressed_bytes={} inspect_ms={}",
        inspection.estimated_stored_size,
        inspect_elapsed.as_millis()
    );
}

#[test]
#[ignore = "manual performance measurement; enumerates 2,000 synthetic assets"]
fn inspects_a_package_with_thousands_of_assets() {
    let package = asset_catalog_package();

    let inspect_started = Instant::now();
    let inspection =
        inspect_file(package.path(), ImportLimits::default()).expect("inspect asset catalog");
    let inspect_elapsed = inspect_started.elapsed();

    assert_eq!(
        usize::try_from(inspection.asset_count).expect("asset count fits in usize"),
        ASSET_CATALOG_SIZE
    );
    eprintln!(
        "scenario=asset_catalog_2000 package_bytes={} assets={} inspect_ms={}",
        fs::metadata(package.path())
            .expect("asset catalog metadata")
            .len(),
        inspection.asset_count,
        inspect_elapsed.as_millis()
    );
}

#[tokio::test]
#[ignore = "manual performance measurement; emits 4,096 synthetic stream chunks"]
async fn processes_a_long_stream() {
    let provider = ChunkedProvider {
        chunks: LONG_STREAM_CHUNKS,
        chunk: LONG_STREAM_CHUNK,
    };
    let (event_sender, mut event_receiver) = mpsc::channel(LONG_STREAM_CHUNKS + 4);
    let (_cancel_sender, cancel_receiver) = watch::channel(false);

    let stream_started = Instant::now();
    let outcome = run_generation(
        &provider,
        synthetic_generation_request(),
        None,
        event_sender,
        cancel_receiver,
    )
    .await
    .expect("complete long stream");
    let stream_elapsed = stream_started.elapsed();

    let events = std::iter::from_fn(|| event_receiver.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.kind, ChatEventKind::TextDelta(_)))
            .count(),
        LONG_STREAM_CHUNKS
    );
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    assert_eq!(
        outcome.text.len(),
        LONG_STREAM_CHUNKS * LONG_STREAM_CHUNK.len()
    );
    eprintln!(
        "scenario=long_stream chunks={} output_bytes={} events={} stream_ms={}",
        LONG_STREAM_CHUNKS,
        outcome.text.len(),
        events.len(),
        stream_elapsed.as_millis()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "manual performance measurement; runs 100 cancel/regenerate cycles"]
async fn rapidly_cancels_and_regenerates() {
    let cycles_started = Instant::now();
    for _ in 0..RAPID_REGENERATION_CYCLES {
        let (event_sender, mut event_receiver) = mpsc::channel(8);
        let (cancel_sender, cancel_receiver) = watch::channel(false);
        let cancellation = tokio::spawn(async move {
            run_generation(
                &CancelAwareProvider,
                synthetic_generation_request(),
                None,
                event_sender,
                cancel_receiver,
            )
            .await
        });

        loop {
            let event = event_receiver
                .recv()
                .await
                .expect("cancelled generation event");
            if matches!(event.kind, ChatEventKind::TextDelta(_)) {
                break;
            }
        }
        cancel_sender.send(true).expect("request cancellation");
        let failure = cancellation
            .await
            .expect("join cancelled generation")
            .expect_err("generation must be cancelled");
        assert_eq!(failure.error.code, CoreErrorCode::Cancelled);
        assert_eq!(failure.partial_text, "partial");

        let (event_sender, _event_receiver) = mpsc::channel(8);
        let (_cancel_sender, cancel_receiver) = watch::channel(false);
        let regenerated = run_generation(
            &StaticProvider::new("regenerated"),
            synthetic_generation_request(),
            None,
            event_sender,
            cancel_receiver,
        )
        .await
        .expect("regenerate after cancellation");
        assert_eq!(regenerated.text, "regenerated");
    }
    let cycles_elapsed = cycles_started.elapsed();
    eprintln!(
        "scenario=rapid_cancel_regenerate cycles={} total_ms={}",
        RAPID_REGENERATION_CYCLES,
        cycles_elapsed.as_millis()
    );
}

#[test]
#[ignore = "manual performance measurement; measures restart restoration and staging cleanup"]
fn recovers_after_an_application_restart() {
    let root = tempdir().expect("temporary data root");
    let card = synthetic_card();
    let (character_id, conversation_id) = {
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let inspection = core.inspect_import(card.path()).expect("inspect");
        let character = core.commit_import(&inspection.id).expect("commit");
        let conversation = core
            .open_conversation(&character.id)
            .expect("open conversation");
        (character.id, conversation.id)
    };

    {
        let storage = Storage::open(root.path()).expect("open storage");
        for index in 0..RECOVERY_MESSAGE_ROWS {
            storage
                .save_message(&Message::user(
                    conversation_id.clone(),
                    format!("restart recovery message {index}"),
                ))
                .expect("save recovery message");
        }
    }

    {
        let core = Core::open(CoreConfig::new(root.path())).expect("open before interruption");
        core.inspect_import(card.path())
            .expect("create abandoned inspection");
    }
    assert!(
        fs::read_dir(root.path().join("staging"))
            .expect("staging directory")
            .next()
            .is_some(),
        "the synthetic interruption must leave staged work"
    );

    let reopen_started = Instant::now();
    let reopened = Core::open(CoreConfig::new(root.path())).expect("recover on reopen");
    let reopen_elapsed = reopen_started.elapsed();
    let restore_started = Instant::now();
    let restored_character = reopened
        .get_character(&character_id)
        .expect("restore character");
    let conversations = reopened
        .list_conversations()
        .expect("restore conversations");
    let messages = reopened
        .list_messages(&conversation_id)
        .expect("restore messages");
    let restore_elapsed = restore_started.elapsed();

    assert_eq!(restored_character.id, character_id);
    assert_eq!(conversations.len(), 1);
    assert_eq!(messages.len(), RECOVERY_MESSAGE_ROWS);
    assert!(
        fs::read_dir(root.path().join("staging"))
            .expect("staging directory")
            .next()
            .is_none(),
        "restart recovery must remove abandoned staging files"
    );
    eprintln!(
        "scenario=restart_recovery messages={} reopen_ms={} restore_ms={}",
        messages.len(),
        reopen_elapsed.as_millis(),
        restore_elapsed.as_millis()
    );
}
