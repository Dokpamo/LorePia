use std::{
    fs::File,
    io::{BufReader, Read},
    path::Path,
};

use lorepia_domain::{CoreError, CoreErrorCode, CoreResult};
use sha2::{Digest, Sha256};

pub fn sha256_file(path: &Path) -> CoreResult<String> {
    let file = File::open(path).map_err(hash_error)?;
    let mut reader = BufReader::new(file);
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(hash_error)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()))
}

fn hash_error(error: std::io::Error) -> CoreError {
    CoreError::new(
        CoreErrorCode::StorageUnavailable,
        format!("cannot hash import source: {error}"),
        true,
    )
}
