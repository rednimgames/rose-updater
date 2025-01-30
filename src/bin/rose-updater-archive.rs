use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::bail;
use clap::Parser;
use path_slash::PathExt;
use tokio::fs;
use tokio::fs::File;
use walkdir::WalkDir;

use rose_update::{RemoteManifest, RemoteManifestFileEntry, CHUNK_SIZE_BYTES};

const REMOTE_MANIFEST_VERSION: usize = 1;

fn parse_compression_level(s: &str) -> Result<u32, String> {
    let err = "Compression level should be a number between 0 and 22";

    let i = match s.parse::<u32>() {
        Ok(i) => i,
        Err(_) => return Err(err.into()),
    };

    if i > 22 {
        return Err(err.into());
    }

    Ok(i)
}

#[derive(Parser, Debug)]
#[clap(about, version, author)]
struct Args {
    /// Input directory
    input: PathBuf,

    /// Output directory
    output: PathBuf,

    /// Relative directory to write archive files to within the output directory
    ///
    /// E.g. If the output path is `/output/` and `archive_prefix_dir` is
    /// `data/` then the archive files will be written to `output/data/`.
    #[clap(long, default_value = "data")]
    archive_prefix_dir: PathBuf,

    /// File extension to use for archive files
    #[clap(long, default_value = "cba")]
    archive_extension: String,

    /// The name to use for the manifest file
    #[clap(long, default_value = "manifest.json")]
    manifest_name: String,

    /// Compression level to use (0 to 22)
    #[clap(long, default_value="4", value_parser=parse_compression_level)]
    compression_level: u32,

    /// Chunk size in bytes
    #[clap(long, default_value_t = CHUNK_SIZE_BYTES)]
    chunk_size: usize,

    /// Relative path to the updater program in the input directory
    #[clap(long, default_value = "rose-updater.exe")]
    updater: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let updater_path = args.input.join(&args.updater);
    if !updater_path.exists() {
        bail!(
            "The updater {} does not exist in the input directory",
            &args.updater.display()
        )
    }

    let mut manifest = RemoteManifest {
        version: REMOTE_MANIFEST_VERSION,
        ..Default::default()
    };

    for entry in WalkDir::new(&args.input).into_iter() {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                let path = err.path().unwrap_or_else(|| Path::new(""));
                eprintln!("Error accessing file {}: {}", path.display(), err);
                continue;
            }
        };

        if !entry.file_type().is_file() {
            continue;
        }

        let input_path = entry.path();
        let input_relative_path = input_path.strip_prefix(&args.input)?;
        let input_extension = input_relative_path
            .extension()
            .unwrap_or_else(|| OsStr::new(""))
            .to_string_lossy();

        let output_relative_path = &args
            .archive_prefix_dir
            .join(input_relative_path)
            .with_extension(format!("{}.{}", &input_extension, &args.archive_extension));

        let output_path = args.output.join(output_relative_path);

        println!("{} => {}", input_path.display(), output_path.display());

        if let Some(output_parent) = output_path.parent() {
            fs::create_dir_all(output_parent).await?;
        }

        let mut input_file = File::open(&input_path).await?;
        let mut output_file = File::create(&output_path).await?;

        let options = bitar::api::compress::CreateArchiveOptions {
            chunker_config: bitar::chunker::Config::FixedSize(args.chunk_size),
            compression: Some(bitar::Compression::zstd(args.compression_level)?),
            ..Default::default()
        };

        let archive_info =
            bitar::api::compress::create_archive(&mut input_file, &mut output_file, &options)
                .await?;

        let entry = RemoteManifestFileEntry {
            path: output_relative_path.to_slash_lossy().to_string(),
            source_path: input_relative_path.to_slash_lossy().to_string(),
            source_hash: archive_info.source_hash,
            source_size: archive_info.source_length,
        };

        if input_path == updater_path {
            manifest.updater = entry;
        } else {
            manifest.files.push(entry);
        }
    }

    let manifest_file = std::fs::File::create(args.output.join(&args.manifest_name))?;
    serde_json::to_writer(manifest_file, &manifest)?;

    Ok(())
}
