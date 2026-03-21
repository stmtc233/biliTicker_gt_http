// main.rs

use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};

use lru::LruCache;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::task;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod abstraction;
mod click;
mod error;
mod slide;
mod w;

use crate::abstraction::{Api, GenerateW, Test, VerifyType};
use crate::click::Click;
use crate::slide::Slide;

#[derive(Clone)]
struct ClientManager {
    clients: Arc<Mutex<LruCache<String, Arc<Client>>>>,
}

const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/119.0.0.0 Safari/537.36";
const CLIENT_CACHE_SIZE: usize = 256;
const INSTANCE_CACHE_SIZE: usize = 127;

impl ClientManager {
    fn new() -> Self {
        Self {
            clients: Arc::new(Mutex::new(LruCache::new(
                NonZeroUsize::new(CLIENT_CACHE_SIZE).unwrap(),
            ))),
        }
    }

    fn get(
        &self,
        proxy: Option<&str>,
        user_agent: Option<&str>,
        referer: Option<&str>,
    ) -> Result<Arc<Client>, crate::error::Error> {
        let proxy_key = proxy.unwrap_or("no_proxy");
        // 使用传入的 user_agent 或默认值来生成缓存键
        let ua_key = user_agent.unwrap_or(DEFAULT_USER_AGENT);
        let referer_key = referer.unwrap_or("no_referer");
        let key = format!("{}|{}|{}", proxy_key, ua_key, referer_key);

        let mut clients = self
            .clients
            .lock()
            .map_err(|_| error::other_without_source("client cache mutex poisoned"))?;
        if let Some(client) = clients.get(&key) {
            return Ok(Arc::clone(client));
        }

        // 确定要设置到客户端上的 User-Agent
        let ua_to_set = user_agent.unwrap_or(DEFAULT_USER_AGENT);

        let mut client_builder = Client::builder()
            .user_agent(ua_to_set)
            // 设置连接超时
            .connect_timeout(Duration::from_secs(10))
            // 设置请求超时
            .timeout(Duration::from_secs(10))
            // 设置连接池空闲超时
            .pool_idle_timeout(Duration::from_secs(10));

        if let Some(referer_to_set) = referer {
            let mut headers = HeaderMap::new();
            let referer_value = HeaderValue::from_str(referer_to_set)
                .map_err(|e| error::other("无效的 Referer", e))?;
            headers.insert(header::REFERER, referer_value);
            client_builder = client_builder.default_headers(headers);
        }

        if let Some(proxy_url) = proxy {
            let proxy =
                reqwest::Proxy::all(proxy_url).map_err(|e| error::other("无效的代理 URL", e))?;
            client_builder = client_builder.proxy(proxy);
        }

        let new_client = client_builder
            .build()
            .map_err(|e| error::other("构建客户端失败", e))?;

        let client_arc = Arc::new(new_client);
        clients.put(key, Arc::clone(&client_arc));
        Ok(client_arc)
    }
}

#[derive(Clone)]
struct AppState {
    client_manager: ClientManager,
    click_instances: Arc<Mutex<LruCache<String, Click>>>,
    slide_instances: Arc<Mutex<LruCache<String, Slide>>>,
}

impl AppState {
    fn new() -> Self {
        let cache_size = NonZeroUsize::new(INSTANCE_CACHE_SIZE).unwrap();
        Self {
            client_manager: ClientManager::new(),
            click_instances: Arc::new(Mutex::new(LruCache::new(cache_size))),
            slide_instances: Arc::new(Mutex::new(LruCache::new(cache_size))),
        }
    }
}

// 统一请求结构体
#[derive(Deserialize)]
struct CommonRequest {
    gt: String,
    challenge: String,
    w: Option<String>,
    session_id: Option<String>,
    proxy: Option<String>,
    user_agent: Option<String>,
    referer: Option<String>,
}

#[derive(Deserialize)]
struct UrlRequest {
    url: String,
    session_id: Option<String>,
    proxy: Option<String>,
    user_agent: Option<String>,
    referer: Option<String>,
}

#[derive(Deserialize)]
struct GenerateWRequest {
    key: String,
    gt: String,
    challenge: String,
    c: Vec<u8>,
    s: String,
    session_id: Option<String>,
    proxy: Option<String>,
    user_agent: Option<String>,
    referer: Option<String>,
}

#[derive(Serialize)]
struct ApiResponse<T> {
    success: bool,
    data: Option<T>,
    error: Option<String>,
}

#[derive(Serialize)]
struct TupleResponse2 {
    first: String,
    second: String,
}

#[derive(Serialize)]
struct CSResponse {
    c: Vec<u8>,
    s: String,
}

impl<T> ApiResponse<T> {
    fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }
    fn error(message: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message),
        }
    }
}

fn get_click_instance(
    state: &AppState,
    session_id: Option<String>,
    proxy: Option<String>,
    user_agent: Option<String>,
    referer: Option<String>,
) -> Result<Click, Response> {
    let session_id = session_id.unwrap_or_else(|| "default".to_string());
    let configured_client = state
        .client_manager
        .get(proxy.as_deref(), user_agent.as_deref(), referer.as_deref())
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::error(e.to_string())),
            )
                .into_response()
        })?;
    let noproxy_client = state.client_manager.get(None, None, None).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        )
            .into_response()
    })?;
    let mut instances = match state.click_instances.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::error(
                    "内部服务错误: Mutex poisoned".to_string(),
                )),
            )
                .into_response())
        }
    };
    if let Some(instance) = instances.get_mut(&session_id) {
        instance.update_client(Arc::clone(&configured_client));
        return Ok(instance.clone());
    }
    let new_instance = Click::new(Arc::clone(&configured_client), Arc::clone(&noproxy_client));
    instances.put(session_id, new_instance.clone());
    Ok(new_instance)
}

fn get_slide_instance(
    state: &AppState,
    session_id: Option<String>,
    proxy: Option<String>,
    user_agent: Option<String>,
    referer: Option<String>,
) -> Result<Slide, Response> {
    let session_id = session_id.unwrap_or_else(|| "default".to_string());
    let configured_client = state
        .client_manager
        .get(proxy.as_deref(), user_agent.as_deref(), referer.as_deref())
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::error(e.to_string())),
            )
                .into_response()
        })?;
    let noproxy_client = state.client_manager.get(None, None, None).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        )
            .into_response()
    })?;
    let mut instances = match state.slide_instances.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::error(
                    "内部服务错误: Mutex poisoned".to_string(),
                )),
            )
                .into_response())
        }
    };
    if let Some(instance) = instances.get_mut(&session_id) {
        instance.update_client(Arc::clone(&configured_client));
        return Ok(instance.clone());
    }
    let new_instance = Slide::new(Arc::clone(&configured_client), Arc::clone(&noproxy_client));
    instances.put(session_id, new_instance.clone());
    Ok(new_instance)
}

// 新增：一个记录请求体的中间件
async fn log_request_body(req: Request<Body>, next: Next) -> Result<Response, StatusCode> {
    if req.method() == axum::http::Method::POST {
        let content_length = req
            .headers()
            .get(axum::http::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("unknown");
        tracing::info!(
            method = %req.method(),
            uri = %req.uri(),
            content_length = %content_length,
            "收到请求"
        );
    }

    Ok(next.run(req).await)
}

macro_rules! handle_blocking_call {
    ($instance_result:expr, $block:expr) => {{
        let mut instance = match $instance_result {
            Ok(inst) => inst,
            Err(resp) => return resp,
        };
        match task::spawn_blocking(move || {
            panic::catch_unwind(AssertUnwindSafe(|| $block(&mut instance)))
        })
        .await
        {
            Ok(Ok(Ok(data))) => Json(ApiResponse::success(data)).into_response(),
            Ok(Ok(Err(e))) => {
                tracing::error!("业务逻辑错误: {}", e);
                (
                    StatusCode::BAD_REQUEST,
                    Json(ApiResponse::<()>::error(e.to_string())),
                )
                    .into_response()
            }
            Ok(Err(e)) => {
                tracing::error!(
                    panic_payload = ?e,
                    "阻塞业务任务发生 panic"
                );
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::<()>::error(
                        "内部服务错误: 阻塞任务 panic".to_string(),
                    )),
                )
                    .into_response()
            }
            Err(e) => {
                tracing::error!("Tokio 任务执行错误: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::<()>::error(e.to_string())),
                )
                    .into_response()
            }
        }
    }};
}

// --- API 处理函数 ---

async fn click_simple_match(
    State(state): State<AppState>,
    Json(req): Json<CommonRequest>,
) -> Response {
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy, req.user_agent, req.referer),
        move |instance: &mut Click| instance.simple_match(&req.gt, &req.challenge)
    )
}

async fn click_simple_match_retry(
    State(state): State<AppState>,
    Json(req): Json<CommonRequest>,
) -> Response {
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy, req.user_agent, req.referer),
        move |instance: &mut Click| instance.simple_match_retry(&req.gt, &req.challenge)
    )
}

async fn click_register_test(
    State(state): State<AppState>,
    Json(req): Json<UrlRequest>,
) -> Response {
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy, req.user_agent, req.referer),
        move |instance: &mut Click| instance
            .register_test(&req.url)
            .map(|(f, s)| TupleResponse2 {
                first: f,
                second: s
            })
    )
}

async fn click_get_c_s(State(state): State<AppState>, Json(req): Json<CommonRequest>) -> Response {
    let w_owned = req.w.clone();
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy, req.user_agent, req.referer),
        move |instance: &mut Click| instance
            .get_c_s(&req.gt, &req.challenge, w_owned.as_deref())
            .map(|(c, s)| CSResponse { c, s })
    )
}

async fn click_get_type(State(state): State<AppState>, Json(req): Json<CommonRequest>) -> Response {
    let w_owned = req.w.clone();
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy, req.user_agent, req.referer),
        move |instance: &mut Click| instance
            .get_type(&req.gt, &req.challenge, w_owned.as_deref())
            .map(|t| match t {
                VerifyType::Click => "click".to_string(),
                VerifyType::Slide => "slide".to_string(),
            })
    )
}

async fn click_verify(State(state): State<AppState>, Json(req): Json<CommonRequest>) -> Response {
    let w_owned = req.w.clone();
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy, req.user_agent, req.referer),
        move |instance: &mut Click| instance
            .verify(&req.gt, &req.challenge, w_owned.as_deref())
            .map(|(f, s)| TupleResponse2 {
                first: f,
                second: s
            })
    )
}

async fn click_generate_w(
    State(state): State<AppState>,
    Json(req): Json<GenerateWRequest>,
) -> Response {
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy, req.user_agent, req.referer),
        move |instance: &mut Click| instance.generate_w(
            &req.key,
            &req.gt,
            &req.challenge,
            &req.c,
            &req.s
        )
    )
}

async fn click_test(State(state): State<AppState>, Json(req): Json<UrlRequest>) -> Response {
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy, req.user_agent, req.referer),
        move |instance: &mut Click| instance.test(&req.url)
    )
}

async fn slide_register_test(
    State(state): State<AppState>,
    Json(req): Json<UrlRequest>,
) -> Response {
    handle_blocking_call!(
        get_slide_instance(&state, req.session_id, req.proxy, req.user_agent, req.referer),
        move |instance: &mut Slide| instance
            .register_test(&req.url)
            .map(|(f, s)| TupleResponse2 {
                first: f,
                second: s
            })
    )
}

async fn slide_get_c_s(State(state): State<AppState>, Json(req): Json<CommonRequest>) -> Response {
    let w_owned = req.w.clone();
    handle_blocking_call!(
        get_slide_instance(&state, req.session_id, req.proxy, req.user_agent, req.referer),
        move |instance: &mut Slide| instance
            .get_c_s(&req.gt, &req.challenge, w_owned.as_deref())
            .map(|(c, s)| CSResponse { c, s })
    )
}

async fn slide_get_type(State(state): State<AppState>, Json(req): Json<CommonRequest>) -> Response {
    let w_owned = req.w.clone();
    handle_blocking_call!(
        get_slide_instance(&state, req.session_id, req.proxy, req.user_agent, req.referer),
        move |instance: &mut Slide| instance
            .get_type(&req.gt, &req.challenge, w_owned.as_deref())
            .map(|t| match t {
                VerifyType::Click => "click".to_string(),
                VerifyType::Slide => "slide".to_string(),
            })
    )
}

async fn slide_verify(State(state): State<AppState>, Json(req): Json<CommonRequest>) -> Response {
    let w_owned = req.w.clone();
    handle_blocking_call!(
        get_slide_instance(&state, req.session_id, req.proxy, req.user_agent, req.referer),
        move |instance: &mut Slide| instance
            .verify(&req.gt, &req.challenge, w_owned.as_deref())
            .map(|(f, s)| TupleResponse2 {
                first: f,
                second: s
            })
    )
}

async fn slide_generate_w(
    State(state): State<AppState>,
    Json(req): Json<GenerateWRequest>,
) -> Response {
    handle_blocking_call!(
        get_slide_instance(&state, req.session_id, req.proxy, req.user_agent, req.referer),
        move |instance: &mut Slide| instance.generate_w(
            &req.key,
            &req.gt,
            &req.challenge,
            &req.c,
            &req.s
        )
    )
}

async fn slide_test(State(state): State<AppState>, Json(req): Json<UrlRequest>) -> Response {
    handle_blocking_call!(
        get_slide_instance(&state, req.session_id, req.proxy, req.user_agent, req.referer),
        move |instance: &mut Slide| instance.test(&req.url)
    )
}

async fn slide_simple_match(
    State(state): State<AppState>,
    Json(req): Json<CommonRequest>,
) -> Response {
    handle_blocking_call!(
        get_slide_instance(&state, req.session_id, req.proxy, req.user_agent, req.referer),
        move |instance: &mut Slide| instance
            .simple_match(&req.gt, &req.challenge)
            .map(|(f, s)| TupleResponse2 {
                first: f,
                second: s
            })
    )
}

async fn health_check() -> &'static str {
    "OK"
}

fn install_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        tracing::error!(panic_info = %panic_info, "检测到未捕获 panic");
        default_hook(panic_info);
    }));
}

#[tokio::main]
async fn main() {
    install_panic_hook();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "bili_ticket_gt_server=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let state = AppState::new();

    let app = Router::new()
        .route("/health", get(health_check))
        .route("/click/simple_match", post(click_simple_match))
        .route("/click/simple_match_retry", post(click_simple_match_retry))
        .route("/click/register_test", post(click_register_test))
        .route("/click/get_c_s", post(click_get_c_s))
        .route("/click/get_type", post(click_get_type))
        .route("/click/verify", post(click_verify))
        .route("/click/generate_w", post(click_generate_w))
        .route("/click/test", post(click_test))
        .route("/slide/register_test", post(slide_register_test))
        .route("/slide/get_c_s", post(slide_get_c_s))
        .route("/slide/get_type", post(slide_get_type))
        .route("/slide/verify", post(slide_verify))
        .route("/slide/generate_w", post(slide_generate_w))
        .route("/slide/test", post(slide_test))
        .route("/slide/simple_match", post(slide_simple_match))
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(middleware::from_fn(log_request_body)) // 应用日志中间件
                .layer(CorsLayer::permissive()),
        )
        .with_state(state);

    let bind_addr = "0.0.0.0:3000";
    let listener = match TcpListener::bind(bind_addr).await {
        Ok(listener) => listener,
        Err(e) => {
            tracing::error!(address = bind_addr, error = %e, "端口绑定失败，服务启动终止");
            std::process::exit(1);
        }
    };

    tracing::info!(address = bind_addr, "HTTP 服务已启动");

    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!(error = %e, "HTTP 监听任务退出，进程即将终止");
        std::process::exit(1);
    }
}
