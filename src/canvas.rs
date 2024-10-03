use crate::config::Config;
use reqwest::header::HeaderMap;
use reqwest::Client;
use reqwest::Response;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Submission {
    #[serde(skip_serializing_if = "Option::is_none")]
    assignment_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    assignment: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    course: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attempt: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    grade: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    grade_matches_current_submission: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    html_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preview_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    submission_comments: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    submission_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    submitted_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    pub user_id: u32,

    #[serde(skip_serializing_if = "Option::is_none")]
    grader_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    graded_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    late: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    assignment_visible: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    excused: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    missing: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    late_policy_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    points_deducted: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seconds_late: Option<u32>,

    pub workflow_state: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    extra_attempts: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    anonymous_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    posted_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    read_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    redo_request: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ScoreUpdate {
    submission: SubmissionScore,
    comment: Comment,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SubmissionScore {
    posted_grade: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Comment {
    text_comment: String,
}

pub struct Canvas {
    pub client: Arc<Client>,
    pub config: Arc<Config>,
    pub url: String,
    pub header: String,
}

impl Canvas {
    const AUTHORIZATION_HEADER: &'static str = "Authorization";

    pub fn new(client: Arc<Client>, config: Arc<Config>) -> Self {
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

    pub async fn get_all_sub(&self) -> Result<Vec<Submission>, Box<dyn std::error::Error>> {
        let mut submissions: Vec<Submission> = Vec::new();
        let mut next_url = Some(self.url.clone());

        while let Some(url) = next_url {
            let response = self
                .client
                .get(&url)
                .header(Self::AUTHORIZATION_HEADER, &self.header)
                .send()
                .await?;

            // resolve next page URL first
            next_url = Self::get_next_link(response.headers(), &self.header);

            // current page submissions
            let page_submissions: Vec<Submission> = response.json().await?;
            submissions.extend(
                page_submissions
                    .into_iter()
                    .filter(|s| s.workflow_state == "submitted"),
            );
        }
        Ok(submissions)
    }

    /// Get next page URL from Link header
    fn get_next_link(headers: &HeaderMap, access_token: &str) -> Option<String> {
        if let Some(link_header) = headers.get("Link") {
            let link_str = link_header.to_str().ok()?;

            // find "rel=\"next\"" part in Link header
            for link in link_str.split(',') {
                if link.contains("rel=\"next\"") {
                    let parts: Vec<&str> = link.split(';').collect();
                    if let Some(url_part) = parts.first() {
                        // re-append access token to the next URL
                        let mut next_url = url_part
                            .trim()
                            .trim_start_matches('<')
                            .trim_end_matches('>')
                            .to_string();
                        if next_url.contains('?') {
                            next_url.push_str(&format!("&access_token={}", access_token));
                        } else {
                            next_url.push_str(&format!("?access_token={}", access_token));
                        }
                        return Some(next_url);
                    }
                }
            }
        }
        None
    }

    pub async fn update_score(
        &self,
        sub_id: u32,
        score: u32,
        comment: &str,
    ) -> Result<Response, Box<dyn std::error::Error>> {
        let url = format!("{}/{}", self.url, sub_id);
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
