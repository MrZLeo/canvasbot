use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub lab_name: String,
    pub api_key: String,
    #[serde(default = "default_api_url")]
    pub api_url: String,
    pub sep_course_id: u32,
    pub lab_assignment_id: u32,
    pub docker_image: String,
    pub docker_cmd: Vec<String>,
    pub lab_timeout: u64,
    #[serde(default = "default_fetch_filter")]
    pub fetch_filter: Vec<String>,
}

fn default_api_url() -> String {
    "https://oc.sjtu.edu.cn".to_string()
}

fn default_fetch_filter() -> Vec<String> {
    vec!["submitted".to_string()]
}
