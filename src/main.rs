mod builtin;
mod canvas;
mod config;
mod worker;

use bollard::container::CreateContainerOptions;
use bollard::container::StartContainerOptions;
use bollard::container::WaitContainerOptions;
use bollard::Docker;
use canvas::Canvas;
use canvas::Submission;
use clap::Parser;
use config::Config;
use futures::StreamExt;
use log::LevelFilter;
use log::{error, info};
use reqwest::Client;
use simple_logger::SimpleLogger;
use std::fs::File;
use std::io::Read;
use std::sync::Arc;
use tokio::time::{interval, timeout, Duration};
use toml::Value;

async fn start_container_runner(docker: Arc<Docker>, canvas: Arc<Canvas>, submission: Submission) {
    let container_name = format!("lab3-{}", submission.user_id);
    let user_id = submission.user_id;
    info!("Start testing for user ID: {}", user_id);

    let attachments = match submission.attachments {
        Some(attachments) => attachments,
        None => {
            let _ = canvas
                .update_score(user_id, 0, "No attachments found")
                .await;
            return;
        }
    };

    let _attachment_url = match attachments.first() {
        Some(attachment) => &attachment.url,
        None => {
            let _ = canvas
                .update_score(user_id, 0, "No attachment URL found")
                .await;
            return;
        }
    };

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
                        .chain([&user_id.to_string()])
                        .map(String::as_str)
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
    let submissions = match canvas
        .get_all_sub(|sub| canvas.config.fetch_filter.contains(&sub.workflow_state))
        .await
    {
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
    Execute {
        #[arg(
            short = 'f',
            long,
            default_value = "config.json",
            help = "Path to the configuration file"
        )]
        config: String,
        #[arg(
            short,
            long,
            default_value = "pipeline.toml",
            help = "Path to the pipeline configuration file"
        )]
        pipeline: String,
        #[arg(short, long, help = "Submission id of the test")]
        sub_id: String,
        #[arg(short, long, help = "URL of the attachment")]
        url: String,
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
    console_subscriber::init();
    let cli = Cli::parse();
    let client = Client::new();

    match cli.command {
        Commands::Daemon { config } => {
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
        Commands::Execute {
            config,
            pipeline,
            sub_id,
            url,
        } => {
            let config = load_config(&config)?;
            let canvas = Arc::new(Canvas::new(Arc::new(client), Arc::new(config)));

            let pipeline = worker::parse_config(&pipeline);
            let mut worker = worker::Worker::new(pipeline.variables);
            for (name, step) in pipeline.steps {
                info!("Adding task: {}", name);
                let task = worker::Task::new(name, step.commands, worker.variables.clone());
                worker.add_task(task);
            }

            // modify the pipeline variables
            worker.modify_variable("url", Value::String(url));

            // Run the pipeline
            worker.run().await;

            info!("Upadting score");
            let final_score = worker
                .variables
                .lock()
                .expect("should finished task")
                .get("score")
                .expect("should have score")
                .clone()
                .expect("should define score")
                .as_integer()
                .expect("should be integer") as u32;
            info!("Final score: {}", final_score);

            let comment = worker
                .results
                .into_iter()
                .map(|(name, res)| res)
                .reduce(|res, msg| res + &msg)
                .expect("should have a result comment");
            println!("Comment:\n{}", comment);

            //canvas
            //    .update_score(sub_id.parse().unwrap(), final_score, &comment)
            //    .await?;
            info!("Pipeline finished");
        }
    }
    Ok(())
}
