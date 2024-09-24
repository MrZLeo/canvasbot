use bollard::container::CreateContainerOptions;
use bollard::container::StartContainerOptions;
use bollard::Docker;
use clap::Parser;
use reqwest::Client;
use reqwest::Response;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Read;
use std::sync::Arc;
use tokio::time::{interval, timeout, Duration};

#[derive(Debug, Serialize, Deserialize)]
struct Config {
    lab_name: String,
    api_key: String,
    #[serde(default = "default_api_url")]
    api_url: String,
    sep_course_id: u32,
    lab_assignment_id: u32,
    docker_image: String,
    docker_cmd: Vec<String>,
    lab_timeout: u64,
}

fn default_api_url() -> String {
    "https://oc.sjtu.edu.cn".to_string()
}

#[derive(Debug, Serialize, Deserialize)]
struct Submission {
    user_id: u32,
    workflow_state: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ScoreUpdate {
    submission: SubmissionScore,
    comment: Comment,
}

#[derive(Debug, Serialize, Deserialize)]
struct SubmissionScore {
    posted_grade: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct Comment {
    text_comment: String,
}

struct Canvas {
    client: Arc<Client>,
    config: Arc<Config>,
    url: String,
    header: String,
}

impl Canvas {
    const AUTHORIZATION_HEADER: &'static str = "Authorization";

    fn new(client: Arc<Client>, config: Arc<Config>) -> Self {
        let url = format!(
            "{}/api/v1/courses/{}/assignments/{}/submissions",
            config.api_url, config.sep_course_id, config.lab_assignment_id
        );
        let header = format!("Bearer {}", config.api_key);
        Self {
            client,
            config,
            url,
            header,
        }
    }

    async fn get_submissions(&self) -> Result<Vec<Submission>, Box<dyn std::error::Error>> {
        let submissions: Vec<Submission> = self
            .client
            .get(&self.url)
            .header(Self::AUTHORIZATION_HEADER, &self.header)
            .send()
            .await?
            .json()
            .await?;
        Ok(submissions)
    }

    async fn update_score(
        &self,
        submission_id: u32,
        score: u32,
        comment: &str,
    ) -> Result<Response, Box<dyn std::error::Error>> {
        let url = format!("{}/{}", self.url, submission_id);
        let score_update = ScoreUpdate {
            submission: SubmissionScore {
                posted_grade: score,
            },
            comment: Comment {
                text_comment: comment.to_string(),
            },
        };

        let response = self
            .client
            .put(&url)
            .header(Self::AUTHORIZATION_HEADER, &self.header)
            .json(&score_update)
            .send()
            .await?;
        Ok(response)
    }
}

async fn start_container_runner(docker: Arc<Docker>, canvas: Arc<Canvas>, submission: Submission) {
    println!("Start testing for user ID: {}", submission.user_id);

    let container = docker
        .create_container(
            Some(CreateContainerOptions {
                name: format!("lab3-{}", submission.user_id),
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
                        .collect(),
                ),
                host_config: Some(bollard::service::HostConfig {
                    memory: Some(1_073_741_824), // 1GB
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await;

    match container {
        Ok(container_info) => {
            let container_id = container_info.id;
            let user_id = submission.user_id;

            match timeout(
                Duration::from_secs(canvas.config.lab_timeout),
                docker.start_container(&container_id, None::<StartContainerOptions<String>>),
            )
            .await
            {
                Ok(_) => {
                    // Container finished within the specified timeout
                    let _ = docker.remove_container(&container_id, None).await;
                    println!("Container for user {} finished successfully", user_id);
                }
                Err(_) => {
                    // Timer triggered, force stop and remove the container
                    let _ = docker.stop_container(&container_id, None).await;
                    let _ = docker.remove_container(&container_id, None).await;
                    let _ = canvas.update_score(user_id, 0, "Test timeout").await;
                    println!("Container for user {} timed out", user_id);
                }
            }
        }
        Err(_) => {
            let _ = canvas
                .update_score(submission.user_id, 0, "Test environment startup error")
                .await;
        }
    }
    println!("Finish {}", submission.user_id);
}

async fn runner(docker: Arc<Docker>, canvas: Arc<Canvas>) {
    let submissions = match canvas.get_submissions().await {
        Ok(subs) => subs,
        Err(e) => {
            eprintln!("Failed to get submissions: {}", e);
            return;
        }
    };

    let mut handles = vec![];

    for submission in submissions {
        if submission.workflow_state == "submitted" {
            let docker = Arc::clone(&docker);
            let canvas = Arc::clone(&canvas);
            let handle = tokio::spawn(async move {
                start_container_runner(docker, canvas, submission).await;
            });
            handles.push(handle);
        }
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
    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon { config } => {
            let client = Client::new();
            let config = load_config(&config)?;
            let canvas = Arc::new(Canvas::new(Arc::new(client), Arc::new(config)));

            let docker = Arc::new(
                Docker::connect_with_local_defaults().expect("Failed to connect to Docker"),
            );

            println!("{} Lab Runner Started", canvas.config.lab_name);

            // Run every 2 minutes
            let mut interval = interval(Duration::from_secs(120));
            loop {
                runner(docker.clone(), canvas.clone()).await;
                interval.tick().await;
            }
        }
        #[allow(unused_variables)]
        Commands::Generate { pipeline, name } => {
            todo!("unfinish yet")
        }
    }
}
