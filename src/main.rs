mod canvas;
mod config;

use canvas::Canvas;
use canvas::Submission;
use config::Config;

use bollard::container::CreateContainerOptions;
use bollard::container::StartContainerOptions;
use bollard::container::WaitContainerOptions;
use bollard::Docker;
use clap::Parser;
use futures::StreamExt;
use log::LevelFilter;
use log::{error, info};
use reqwest::Client;
use simple_logger::SimpleLogger;
use std::fs::File;
use std::io::Read;
use std::sync::Arc;
use tokio::time::{interval, timeout, Duration};

async fn start_container_runner(docker: Arc<Docker>, canvas: Arc<Canvas>, submission: Submission) {
    info!("Start testing for user ID: {}", submission.user_id);

    let container_name = format!("lab3-{}", submission.user_id);
    let user_id = submission.user_id;
    if docker
        .create_container(
            Some(CreateContainerOptions {
                name: &container_name,
                platform: None,
            }),
            bollard::container::Config {
                image: Some(canvas.config.docker_image.as_str()),
                cmd: Some(
                    canvas
                        .config
                        .docker_cmd
                        .iter()
                        .map(|s| s.as_str())
                        .chain(std::iter::once(user_id.to_string().as_str()))
                        .collect(),
                ),
                host_config: Some(bollard::service::HostConfig {
                    memory: Some(1_073_741_824), // 1GB
                    auto_remove: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .is_err()
    {
        let _ = canvas
            .update_score(user_id, 0, "Test environment startup error")
            .await;
    };

    info!("Container {} created", container_name);

    // Start the container
    if (docker
        .start_container(&container_name, None::<StartContainerOptions<String>>)
        .await)
        .is_err()
    {
        let _ = canvas
            .update_score(user_id, 0, "Failed to start container")
            .await;
        return;
    }

    // Wait for container
    let wait_options = WaitContainerOptions {
        condition: "not-running".to_string(),
    };
    let mut wait_stream = docker.wait_container::<String>(&container_name, Some(wait_options));
    match timeout(
        Duration::from_secs(canvas.config.lab_timeout),
        wait_stream.next(),
    )
    .await
    {
        Ok(Some(Ok(_))) => {
            info!("Container for user {} finished successfully", user_id);
        }
        Ok(Some(Err(e))) => {
            error!("Error waiting for container: {:?}", e);
        }
        Ok(None) => {
            error!("wait_container stream ended unexpectedly");
        }
        Err(_) => {
            // Test timeout
            error!("Container for user {} timed out", user_id);
            if let Err(e) = docker.stop_container(&container_name, None).await {
                error!("Error stopping container: {:?}", e);
            }
            if let Err(e) = docker.remove_container(&container_name, None).await {
                error!("Error removing container: {:?}", e);
            }
            if let Err(e) = canvas.update_score(user_id, 0, "Test timeout").await {
                error!("Error updating score: {:?}", e);
            }
        }
    }

    info!("Finish {}", submission.user_id);
}

async fn runner(docker: Arc<Docker>, canvas: Arc<Canvas>) {
    let submissions = match canvas.get_all_sub().await {
        Ok(subs) => subs,
        Err(e) => {
            error!("Failed to get submissions: {}", e);
            return;
        }
    };

    let mut handles = vec![];

    for submission in submissions {
        let docker = Arc::clone(&docker);
        let canvas = Arc::clone(&canvas);
        let handle = tokio::spawn(async move {
            start_container_runner(docker, canvas, submission).await;
        });
        handles.push(handle);
    }

    for handle in handles {
        handle
            .await
            .unwrap_or_else(|e| eprintln!("Task failed: {:?}", e));
    }
}

fn load_config(config_path: &str) -> Result<Config, Box<dyn std::error::Error>> {
    let mut file = File::open(config_path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    let config: Config = serde_json::from_str(&contents)?;
    validate_config(&config)?;
    Ok(config)
}

fn validate_config(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    if config.lab_name.is_empty() {
        return Err("LAB_NAME is empty in config.json".into());
    }
    if config.api_key.is_empty() {
        return Err("API_KEY is empty in config.json".into());
    }
    if config.api_url.is_empty() {
        return Err("API_URL is empty in config.json".into());
    }
    if config.sep_course_id == 0 {
        return Err("SEP_COURSE_ID is not set in config.json".into());
    }
    if config.lab_assignment_id == 0 {
        return Err("LAB3_ASSIGNMENT_ID is not set in config.json".into());
    }
    if config.docker_image.is_empty() {
        return Err("DOCKER_IMAGE is not set in config.json".into());
    }
    if config.docker_cmd.is_empty() {
        return Err("DOCKER_CMD is not set in config.json".into());
    }
    if config.lab_timeout == 0 {
        return Err("LAB_TIMEOUT is not set or is zero in config.json".into());
    }
    Ok(())
}

#[derive(Parser, Debug)]
#[command(
    name = "canvasbot",
    version = "1.0",
    author = "Zhang, Jing <mrzleo@foxmail.com>",
    about = "Runs lab assignments in Docker containers",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand, Debug)]
enum Commands {
    Daemon {
        #[arg(
            short = 'f',
            long,
            default_value = "config.json",
            help = "Path to the configuration file"
        )]
        config: String,
    },
    Generate {
        #[arg(
            short,
            long,
            default_value = "pipeline.yaml",
            help = "Path to the pipeline configuration file"
        )]
        pipeline: String,
        #[arg(
            short,
            long,
            default_value = "lab_docker_runner",
            help = "Name of the Docker runner"
        )]
        name: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    SimpleLogger::new()
        .with_level(LevelFilter::Info)
        .with_colors(true)
        .without_timestamps()
        .init()
        .unwrap();
    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon { config } => {
            let client = Client::new();
            let config = load_config(&config)?;
            let canvas = Arc::new(Canvas::new(Arc::new(client), Arc::new(config)));

            let docker = Arc::new(
                Docker::connect_with_local_defaults().expect("Failed to connect to Docker"),
            );

            info!("{} Lab Runner Started", canvas.config.lab_name);

            // Run every 2 minutes
            let mut interval = interval(Duration::from_secs(120));
            loop {
                runner(docker.clone(), canvas.clone()).await;
                interval.tick().await;
            }
        }
        #[allow(unused_variables)]
        Commands::Generate { pipeline, name } => {
            unimplemented!()
        }
    }
}
