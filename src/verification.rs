//! Verify installed bytes against the manifest, not only the downloaded archive.
use std::path::Path;

use anyhow::{ensure, Context};
use blake2::{Blake2b512, Digest};
use tokio::io::AsyncReadExt;

pub async fn verify_file(
    path: &Path,
    expected_size: u64,
    expected_hash: &[u8],
) -> anyhow::Result<()> {
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("Cannot verify {}", path.display()))?;
    ensure!(
        file.metadata().await?.len() == expected_size,
        "Update size mismatch: {}. Run Repair before launching.",
        path.display()
    );
    let mut hasher = Blake2b512::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    ensure!(
        hasher.finalize().as_slice() == expected_hash,
        "Update hash mismatch: {}. Run Repair before launching.",
        path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_another_release_even_when_size_matches() {
        let path = std::env::temp_dir().join(format!("rose-verify-{}.bin", std::process::id()));
        let expected = Blake2b512::digest(b"release-A");
        tokio::fs::write(&path, b"release-B").await.unwrap();
        assert!(verify_file(&path, 9, &expected)
            .await
            .unwrap_err()
            .to_string()
            .contains("hash mismatch"));
        assert!(verify_file(&path, 8, &expected)
            .await
            .unwrap_err()
            .to_string()
            .contains("size mismatch"));
        tokio::fs::write(&path, b"release-A").await.unwrap();
        verify_file(&path, 9, &expected).await.unwrap();
        tokio::fs::remove_file(path).await.unwrap();
    }
}
