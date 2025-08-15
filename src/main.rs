// main.rs

use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::task;
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
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
    clients: Arc<Mutex<HashMap<String, Arc<Client>>>>,
}

// --- 修改开始 ---

const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/119.0.0.0 Safari/537.36";

impl ClientManager {
    fn new() -> Self {
        Self {
            clients: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn get(&self, proxy: Option<&str>, user_agent: Option<&str>) -> Result<Arc<Client>, crate::error::Error> {
        let proxy_key = proxy.unwrap_or("no_proxy");
        // 使用传入的 user_agent 或默认值来生成缓存键
        let ua_key = user_agent.unwrap_or(DEFAULT_USER_AGENT);
        let key = format!("{}|{}", proxy_key, ua_key);

        let mut clients = self.clients.lock().expect("ClientManager mutex poisoned");
        if let Some(client) = clients.get(&key) {
            return Ok(Arc::clone(client));
        }

        // 确定要设置到客户端上的 User-Agent
        let ua_to_set = user_agent.unwrap_or(DEFAULT_USER_AGENT);

        let mut client_builder = Client::builder()
            .user_agent(ua_to_set); // 总是设置 User-Agent

        if let Some(proxy_url) = proxy {
            let proxy = reqwest::Proxy::all(proxy_url)
                .map_err(|e| error::other("无效的代理 URL", e))?;
            client_builder = client_builder.proxy(proxy);
        }

        let new_client = client_builder
            .build()
            .map_err(|e| error::other("构建客户端失败", e))?;

        let client_arc = Arc::new(new_client);
        clients.insert(key, Arc::clone(&client_arc));
        Ok(client_arc)
    }
}

// --- 修改结束 ---

#[derive(Clone)]
struct AppState {
    client_manager: ClientManager,
    click_instances: Arc<Mutex<HashMap<String, Click>>>,
    slide_instances: Arc<Mutex<HashMap<String, Slide>>>,
}
impl AppState {
    fn new() -> Self {
        Self {
            client_manager: ClientManager::new(),
            click_instances: Arc::new(Mutex::new(HashMap::new())),
            slide_instances: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}
#[derive(Deserialize)]
struct SimpleMatchRequest {
    gt: String,
    challenge: String,
    session_id: Option<String>,
    proxy: Option<String>,
    user_agent: Option<String>,
}
#[derive(Deserialize)]
struct RegisterTestRequest {
    url: String,
    session_id: Option<String>,
    proxy: Option<String>,
    user_agent: Option<String>,
}
#[derive(Deserialize)]
struct GetCSRequest {
    gt: String,
    challenge: String,
    w: Option<String>,
    session_id: Option<String>,
    proxy: Option<String>,
    user_agent: Option<String>,
}
#[derive(Deserialize)]
struct GetTypeRequest {
    gt: String,
    challenge: String,
    w: Option<String>,
    session_id: Option<String>,
    proxy: Option<String>,
    user_agent: Option<String>,
}
#[derive(Deserialize)]
struct VerifyRequest {
    gt: String,
    challenge: String,
    w: Option<String>,
    session_id: Option<String>,
    proxy: Option<String>,
    user_agent: Option<String>,
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
}
#[derive(Deserialize)]
struct TestRequest {
    url: String,
    session_id: Option<String>,
    proxy: Option<String>,
    user_agent: Option<String>,
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
        Self { success: true, data: Some(data), error: None }
    }
    fn error(message: String) -> Self {
        Self { success: false, data: None, error: Some(message) }
    }
}
fn get_click_instance(
    state: &AppState,
    session_id: Option<String>,
    proxy: Option<String>,
    user_agent: Option<String>,
) -> Result<Click, Response> {
    let session_id = session_id.unwrap_or_else(|| "default".to_string());
    let configured_client = state.client_manager.get(proxy.as_deref(), user_agent.as_deref()).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error(e.to_string()))).into_response()
    })?;
    // noproxy_client 现在也会有一个默认的 User-Agent
    let noproxy_client = state.client_manager.get(None, None).map_err(|e| {
         (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error(e.to_string()))).into_response()
    })?;
    let mut instances = match state.click_instances.lock() {
        Ok(guard) => guard,
        Err(_) => return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error("内部服务错误: Mutex poisoned".to_string()))).into_response()),
    };
    let instance = instances
        .entry(session_id)
        .or_insert_with(|| Click::new(Arc::clone(&configured_client), Arc::clone(&noproxy_client)));
    instance.update_client(Arc::clone(&configured_client));
    Ok(instance.clone())
}
fn get_slide_instance(
    state: &AppState,
    session_id: Option<String>,
    proxy: Option<String>,
    user_agent: Option<String>,
) -> Result<Slide, Response> {
    let session_id = session_id.unwrap_or_else(|| "default".to_string());
    let configured_client = state.client_manager.get(proxy.as_deref(), user_agent.as_deref()).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error(e.to_string()))).into_response()
    })?;
    // noproxy_client 现在也会有一个默认的 User-Agent
    let noproxy_client = state.client_manager.get(None, None).map_err(|e| {
         (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error(e.to_string()))).into_response()
    })?;
    let mut instances = match state.slide_instances.lock() {
        Ok(guard) => guard,
        Err(_) => return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error("内部服务错误: Mutex poisoned".to_string()))).into_response()),
    };
    let instance = instances
        .entry(session_id)
        .or_insert_with(|| Slide::new(Arc::clone(&configured_client), Arc::clone(&noproxy_client)));
    instance.update_client(Arc::clone(&configured_client));
    Ok(instance.clone())
}

// 新增：一个记录请求体的中间件
async fn log_request_body(req: Request<Body>, next: Next) -> Result<Response, StatusCode> {
    let (parts, body) = req.into_parts();

    if parts.method == "POST" {
        let bytes = match axum::body::to_bytes(body, usize::MAX).await {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::error!("无法读取请求 body: {}", err);
                return Err(StatusCode::BAD_REQUEST);
            }
        };
        if let Ok(body_str) = std::str::from_utf8(&bytes) {
            tracing::info!(method = %parts.method, uri = %parts.uri, body = %body_str, "收到请求");
        } else {
            tracing::info!(method = %parts.method, uri = %parts.uri, body = "[非 UTF-8 数据]", "收到请求");
        }
        let req = Request::from_parts(parts, Body::from(bytes));
        Ok(next.run(req).await)
    } else {
        let req = Request::from_parts(parts, body);
        Ok(next.run(req).await)
    }
}

macro_rules! handle_blocking_call {
    ($instance_result:expr, $block:expr) => {
        {
            let mut instance = match $instance_result {
                Ok(inst) => inst,
                Err(resp) => return resp,
            };
            match task::spawn_blocking(move || $block(&mut instance)).await {
                Ok(Ok(data)) => Json(ApiResponse::success(data)).into_response(),
                Ok(Err(e)) => {
                    tracing::error!("业务逻辑错误: {}", e);
                    (StatusCode::BAD_REQUEST, Json(ApiResponse::<()>::error(e.to_string()))).into_response()
                },
                Err(e) => {
                    tracing::error!("Tokio 任务执行错误: {}", e);
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error(e.to_string()))).into_response()
                },
            }
        }
    };
}


// --- API 处理函数 (保持不变) ---
async fn click_simple_match(State(state): State<AppState>, Json(req): Json<SimpleMatchRequest>) -> Response {
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy, req.user_agent),
        move |instance: &mut Click| instance.simple_match(&req.gt, &req.challenge)
    )
}

async fn click_simple_match_retry(State(state): State<AppState>, Json(req): Json<SimpleMatchRequest>) -> Response {
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy, req.user_agent),
        move |instance: &mut Click| instance.simple_match_retry(&req.gt, &req.challenge)
    )
}

async fn click_register_test(State(state): State<AppState>, Json(req): Json<RegisterTestRequest>) -> Response {
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy, req.user_agent),
        move |instance: &mut Click| instance.register_test(&req.url).map(|(f, s)| TupleResponse2 { first: f, second: s })
    )
}

async fn click_get_c_s(State(state): State<AppState>, Json(req): Json<GetCSRequest>) -> Response {
    let w_owned = req.w.clone();
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy, req.user_agent),
        move |instance: &mut Click| instance.get_c_s(&req.gt, &req.challenge, w_owned.as_deref()).map(|(c, s)| CSResponse { c, s })
    )
}

async fn click_get_type(State(state): State<AppState>, Json(req): Json<GetTypeRequest>) -> Response {
    let w_owned = req.w.clone();
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy, req.user_agent),
        move |instance: &mut Click| instance.get_type(&req.gt, &req.challenge, w_owned.as_deref()).map(|t| match t {
            VerifyType::Click => "click".to_string(),
            VerifyType::Slide => "slide".to_string(),
        })
    )
}

async fn click_verify(State(state): State<AppState>, Json(req): Json<VerifyRequest>) -> Response {
    let w_owned = req.w.clone();
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy, req.user_agent),
        move |instance: &mut Click| instance.verify(&req.gt, &req.challenge, w_owned.as_deref()).map(|(f, s)| TupleResponse2 { first: f, second: s })
    )
}

async fn click_generate_w(State(state): State<AppState>, Json(req): Json<GenerateWRequest>) -> Response {
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy, req.user_agent),
        move |instance: &mut Click| instance.generate_w(&req.key, &req.gt, &req.challenge, &req.c, &req.s)
    )
}

async fn click_test(State(state): State<AppState>, Json(req): Json<TestRequest>) -> Response {
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy, req.user_agent),
        move |instance: &mut Click| instance.test(&req.url)
    )
}

async fn slide_register_test(State(state): State<AppState>, Json(req): Json<RegisterTestRequest>) -> Response {
    handle_blocking_call!(
        get_slide_instance(&state, req.session_id, req.proxy, req.user_agent),
        move |instance: &mut Slide| instance.register_test(&req.url).map(|(f, s)| TupleResponse2 { first: f, second: s })
    )
}

async fn slide_get_c_s(State(state): State<AppState>, Json(req): Json<GetCSRequest>) -> Response {
    let w_owned = req.w.clone();
    handle_blocking_call!(
        get_slide_instance(&state, req.session_id, req.proxy, req.user_agent),
        move |instance: &mut Slide| instance.get_c_s(&req.gt, &req.challenge, w_owned.as_deref()).map(|(c, s)| CSResponse { c, s })
    )
}

async fn slide_get_type(State(state): State<AppState>, Json(req): Json<GetTypeRequest>) -> Response {
    let w_owned = req.w.clone();
    handle_blocking_call!(
        get_slide_instance(&state, req.session_id, req.proxy, req.user_agent),
        move |instance: &mut Slide| instance.get_type(&req.gt, &req.challenge, w_owned.as_deref()).map(|t| match t {
            VerifyType::Click => "click".to_string(),
            VerifyType::Slide => "slide".to_string(),
        })
    )
}

async fn slide_verify(State(state): State<AppState>, Json(req): Json<VerifyRequest>) -> Response {
    let w_owned = req.w.clone();
    handle_blocking_call!(
        get_slide_instance(&state, req.session_id, req.proxy, req.user_agent),
        move |instance: &mut Slide| instance.verify(&req.gt, &req.challenge, w_owned.as_deref()).map(|(f, s)| TupleResponse2 { first: f, second: s })
    )
}

async fn slide_generate_w(State(state): State<AppState>, Json(req): Json<GenerateWRequest>) -> Response {
    handle_blocking_call!(
        get_slide_instance(&state, req.session_id, req.proxy, req.user_agent),
        move |instance: &mut Slide| instance.generate_w(&req.key, &req.gt, &req.challenge, &req.c, &req.s)
    )
}

async fn slide_test(State(state): State<AppState>, Json(req): Json<TestRequest>) -> Response {
    handle_blocking_call!(
        get_slide_instance(&state, req.session_id, req.proxy, req.user_agent),
        move |instance: &mut Slide| instance.test(&req.url)
    )
}

async fn health_check() -> &'static str {
    "OK"
}

#[tokio::main]
async fn main() {
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
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(middleware::from_fn(log_request_body)) // 应用日志中间件
                .layer(CorsLayer::permissive()),
        )
        .with_state(state);

    let listener = TcpListener::bind("0.0.0.0:3000").await.unwrap();
    
    tracing::info!("服务已启动于 http://0.0.0.0:3000");
    
    axum::serve(listener, app).await.unwrap();
}