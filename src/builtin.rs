use futures::future::BoxFuture;
use similar::{ChangeTag, TextDiff};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::{error::Error, fs::read_to_string};

pub struct BuiltinRegistry {
    commands: HashMap<String, BuiltinFn>,
}

type BuiltinFn =
    Arc<dyn Fn(Vec<String>) -> BoxFuture<'static, Result<(), Box<dyn Error>>> + Send + Sync>;

impl BuiltinRegistry {
    pub fn new() -> Self {
        Self {
            commands: HashMap::new(),
        }
    }

    pub fn register<F>(&mut self, name: &str, function: F) -> &mut Self
    where
        F: Fn(Vec<String>) -> BoxFuture<'static, Result<(), Box<dyn Error>>>
            + Send
            + Sync
            + 'static,
    {
        self.commands.insert(name.to_string(), Arc::new(function));
        self
    }

    pub async fn execute(&self, name: &str, args: Vec<String>) -> Result<(), Box<dyn Error>> {
        if let Some(command) = self.commands.get(name) {
            command(args).await
        } else {
            Err(format!("Builtin command '{}' not found.", name).into())
        }
    }
}

pub fn create_builtin_registry() -> BuiltinRegistry {
    let mut registry = BuiltinRegistry::new();
    registry
        .register("download_and_extract_7z", |args| {
            Box::pin(download_and_extract_7z_builtin(args))
        })
        .register("diff_file", |args| Box::pin(diff_file_builtin(args)))
        .register("compile_cmake", |args| {
            Box::pin(compile_cmake_builtin(args))
        });

    registry
}

async fn download_and_extract_7z_builtin(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    let url = args.first().ok_or("URL not set in arguments")?;
    let output = args.get(1).ok_or("output dir not set in arguments")?;
    download_and_extract_7z(url, output).await
}

async fn download_and_extract_7z(url: &str, output_dir: &str) -> Result<(), Box<dyn Error>> {
    // Local path to save the downloaded file
    let downloaded_path = Path::new(output_dir).join("submitted.7z");

    // Directory to extract the contents
    let extract_dir = Path::new(output_dir);
    if extract_dir.exists() {
        std::fs::remove_dir_all(extract_dir)?;
    }
    std::fs::create_dir_all(extract_dir)?;

    // Download the file
    let response = reqwest::get(url).await?;
    let content = response.bytes().await?;
    let mut dest = File::create(&downloaded_path)?;
    dest.write_all(&content)?;

    // Extract the 7z file
    sevenz_rust::decompress_file(&downloaded_path, extract_dir)?;

    Ok(())
}

async fn diff_file_builtin(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    let base = args.first().ok_or("Base file not set")?;
    let submission = args.get(1).ok_or("Submission file not set")?;
    let count = args.get(2).ok_or("Count not specify")?;

    let diff = diff_file(base, submission).await?;
    if diff > count.parse()? {
        return Err(format!("Diff count {} is greater than {}", diff, count).into());
    }

    Ok(())
}

/// return: line diff count
async fn diff_file(base: &str, submission: &str) -> Result<usize, Box<dyn Error>> {
    let old = read_to_string(base)?;
    let new = read_to_string(submission)?;
    let diff = TextDiff::from_lines(&old, &new)
        .iter_all_changes()
        .filter(|change| change.tag() != ChangeTag::Equal)
        .count();
    Ok(diff)
}

async fn compile_cmake_builtin(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    let dir = args.first().ok_or("Directory not set")?;

    compile_cmake(dir).await
}

/// Compile Cmake in the given directory
async fn compile_cmake(dir: &str) -> Result<(), Box<dyn Error>> {
    Command::new("cmake")
        .args(["-B", "build", "-S", dir])
        .status()?;

    Command::new("cmake").args(["--build", "build"]).status()?;

    Ok(())
}
