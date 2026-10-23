mod image_match;
mod protocol;

use crate::error::{other, other_without_source, Result};
use image_match::{decode_background, decode_question, IconRecognizer};
use protocol::{LoadData, Seccode, V4Client};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_MAX_ATTEMPTS: usize = 6;
const MAX_ATTEMPTS_CAP: usize = 10;
const MIN_VERIFY_DELAY: Duration = Duration::from_millis(1800);
const FAILED_ATTEMPT_DELAY: Duration = Duration::from_millis(1200);

#[derive(Debug, Clone, Deserialize)]
pub struct SolveRequest {
    pub captcha_id: String,
    #[serde(default)]
    pub challenge: String,
    #[serde(default = "default_risk_type")]
    pub risk_type: String,
    #[serde(default)]
    pub max_attempts: Option<usize>,
    #[serde(default)]
    pub proxy: Option<String>,
}

fn default_risk_type() -> String {
    "icon".to_string()
}

#[derive(Debug, Clone, Serialize)]
pub struct SolveResult {
    pub lot_number: String,
    pub captcha_output: String,
    pub pass_token: String,
    pub gen_time: String,
    pub attempts: usize,
    pub elapsed_ms: f64,
    pub candidate_count: usize,
}

#[derive(Debug, Default)]
struct Metrics {
    solved: u64,
    failed: u64,
}

pub struct V4Service {
    recognizer: IconRecognizer,
    metrics: Mutex<Metrics>,
    started_at: Instant,
}

impl V4Service {
    pub fn load(model_dir: &Path, concurrency: usize, threads: usize) -> Result<Self> {
        let recognizer = IconRecognizer::load(model_dir, concurrency.clamp(1, 4), threads)?;
        recognizer.warm_up()?;
        Ok(Self {
            recognizer,
            metrics: Mutex::new(Metrics::default()),
            started_at: Instant::now(),
        })
    }

    pub fn health(&self) -> (u64, u64, f64) {
        let metrics = self
            .metrics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            metrics.solved,
            metrics.failed,
            self.started_at.elapsed().as_secs_f64(),
        )
    }

    fn record(&self, success: bool) {
        let mut metrics = self
            .metrics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if success {
            metrics.solved += 1;
        } else {
            metrics.failed += 1;
        }
    }

    pub fn solve(&self, request: SolveRequest) -> Result<SolveResult> {
        match self.solve_inner(request) {
            Ok(result) => {
                self.record(true);
                Ok(result)
            }
            Err(error) => {
                self.record(false);
                Err(error)
            }
        }
    }

    fn solve_inner(&self, request: SolveRequest) -> Result<SolveResult> {
        let started = Instant::now();
        if request.captcha_id.trim().is_empty() {
            return Err(other_without_source("geetest4 captcha_id is empty"));
        }
        let max_attempts = request
            .max_attempts
            .unwrap_or(DEFAULT_MAX_ATTEMPTS)
            .clamp(1, MAX_ATTEMPTS_CAP);
        let request_id = random_request_id();
        let client = V4Client::new(request.proxy.as_deref())?;
        let mut data = client.load(
            &request.captcha_id,
            &request.challenge,
            &request.risk_type,
            true,
        )?;
        if data.captcha_type != "icon" {
            return Err(other_without_source(&format!(
                "unsupported GeeTest v4 type: {}",
                data.captcha_type
            )));
        }
        if data.ques.len() != 3 {
            return Err(other_without_source(&format!(
                "expected 3 question icons, got {}",
                data.ques.len()
            )));
        }

        let background_bytes = client.load_resource(&data.imgs, true)?;
        let mut questions = Vec::with_capacity(data.ques.len());
        for path in &data.ques {
            questions.push(decode_question(&client.load_resource(path, true)?)?);
        }
        let background = decode_background(&background_bytes)?;
        let loaded_at = Instant::now();
        tracing::info!(
            request_id = %request_id,
            width = background.width(),
            height = background.height(),
            questions = questions.len(),
            "GeeTest v4 challenge loaded"
        );

        let primary = self
            .recognizer
            .primary_plan(&background, &questions, max_attempts.min(2))?;
        tracing::info!(
            request_id = %request_id,
            phase = primary.phase,
            candidates = primary.candidate_count,
            ranked_click_sets = primary.clicks.len(),
            "GeeTest v4 primary recognition complete"
        );
        let mut attempts = 0usize;
        let mut used_clicks = HashSet::new();
        let mut candidate_count = primary.candidate_count;

        if primary.clicks.is_empty() {
            return Err(other_without_source("v4 recognizer returned no candidates"));
        }

        let first_plan_result = self.try_plan(
            &client,
            &request,
            &mut data,
            primary.clicks,
            "primary",
            &mut attempts,
            max_attempts,
            loaded_at,
            &mut used_clicks,
        )?;
        if let Some(seccode) = first_plan_result {
            return Ok(complete_result(seccode, attempts, started, candidate_count));
        }
        if attempts >= max_attempts {
            return Err(other_without_source(&format!(
                "geetest4 solver exhausted {attempts} attempts"
            )));
        }

        let fallback_limit = max_attempts.saturating_sub(attempts);
        let fallback = self
            .recognizer
            .fallback_plan(&background, &questions, fallback_limit)?;
        tracing::info!(
            request_id = %request_id,
            phase = fallback.phase,
            candidates = fallback.candidate_count,
            ranked_click_sets = fallback.clicks.len(),
            "GeeTest v4 fallback recognition complete"
        );
        candidate_count = candidate_count.max(fallback.candidate_count);
        let fallback_result = self.try_plan(
            &client,
            &request,
            &mut data,
            fallback.clicks,
            "fallback",
            &mut attempts,
            max_attempts,
            loaded_at,
            &mut used_clicks,
        )?;
        if let Some(seccode) = fallback_result {
            return Ok(complete_result(seccode, attempts, started, candidate_count));
        }
        Err(other_without_source(&format!(
            "geetest4 solver exhausted {attempts} attempts"
        )))
    }

    #[allow(clippy::too_many_arguments)]
    fn try_plan(
        &self,
        client: &V4Client,
        request: &SolveRequest,
        data: &mut LoadData,
        clicks: Vec<Vec<(f64, f64)>>,
        phase: &str,
        attempts: &mut usize,
        max_attempts: usize,
        loaded_at: Instant,
        used_clicks: &mut HashSet<Vec<(i64, i64)>>,
    ) -> Result<Option<Seccode>> {
        for click_set in clicks {
            if *attempts >= max_attempts {
                break;
            }
            let signature = click_set
                .iter()
                .map(|(x, y)| ((x * 10_000.0).round() as i64, (y * 10_000.0).round() as i64))
                .collect::<Vec<_>>();
            let mut sorted_signature = signature.clone();
            sorted_signature.sort_unstable();
            if !used_clicks.insert(sorted_signature) {
                continue;
            }
            if *attempts == 0 {
                let elapsed = loaded_at.elapsed();
                if elapsed < MIN_VERIFY_DELAY {
                    std::thread::sleep(MIN_VERIFY_DELAY - elapsed);
                }
            }
            *attempts += 1;
            let verify = client.verify(
                &request.captcha_id,
                &request.risk_type,
                data,
                &click_set,
                true,
            )?;
            tracing::info!(
                attempt = *attempts,
                max_attempts,
                phase,
                result = %verify.result,
                fail_count = ?verify.fail_count,
                "GeeTest v4 verify attempt"
            );
            if verify.result == "success" {
                let seccode = verify.seccode.ok_or_else(|| {
                    other_without_source("GeeTest v4 success response missing seccode")
                })?;
                if seccode.lot_number.is_empty()
                    || seccode.captcha_output.is_empty()
                    || seccode.pass_token.is_empty()
                    || seccode.gen_time.is_empty()
                {
                    return Err(other_without_source(
                        "GeeTest v4 success response has incomplete credentials",
                    ));
                }
                return Ok(Some(seccode));
            }
            if verify.result == "forbidden" {
                return Err(other_without_source("geetest4 solver was forbidden"));
            }
            if let Some(payload) = verify.payload {
                data.payload = payload;
            }
            if let Some(process_token) = verify.process_token {
                data.process_token = process_token;
            }
            if *attempts < max_attempts {
                std::thread::sleep(FAILED_ATTEMPT_DELAY);
            }
        }
        Ok(None)
    }
}

fn complete_result(
    seccode: Seccode,
    attempts: usize,
    started: Instant,
    candidate_count: usize,
) -> SolveResult {
    SolveResult {
        lot_number: seccode.lot_number,
        captcha_output: seccode.captcha_output,
        pass_token: seccode.pass_token,
        gen_time: seccode.gen_time,
        attempts,
        elapsed_ms: (started.elapsed().as_secs_f64() * 1000.0 * 10.0).round() / 10.0,
        candidate_count,
    }
}

fn random_request_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{now:x}-{:x}", std::process::id())
}

pub fn model_dir_from_env() -> std::path::PathBuf {
    std::env::var_os("GT4_MODEL_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("models"))
}

pub fn env_usize(name: &str, default: usize, min: usize, max: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default)
        .clamp(min, max)
}

#[allow(dead_code)]
fn _assert_error_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<V4Service>();
    let _ = other("", std::io::Error::other(""));
}
