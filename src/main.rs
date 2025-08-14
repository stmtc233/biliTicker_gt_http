// main.rs

use axum::{
    extract::State,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::task;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;

// 引入你的模块
mod abstraction;
mod click;
mod error;
mod slide;
mod w;

use crate::abstraction::{Api, GenerateW, Test, VerifyType};
use crate::click::Click;
use crate::slide::Slide;

// 应用状态
#[derive(Clone)]
struct AppState {
    click_instances: Arc<Mutex<HashMap<String, Click>>>,
    slide_instances: Arc<Mutex<HashMap<String, Slide>>>,
}

impl AppState {
    fn new() -> Self {
        Self {
            click_instances: Arc::new(Mutex::new(HashMap::new())),
            slide_instances: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

// --- 请求和响应结构体 ---
#[derive(Deserialize)]
struct SimpleMatchRequest {
    gt: String,
    challenge: String,
    session_id: Option<String>,
    proxy: Option<String>,
}

#[derive(Deserialize)]
struct RegisterTestRequest {
    url: String,
    session_id: Option<String>,
    proxy: Option<String>,
}

#[derive(Deserialize)]
struct GetCSRequest {
    gt: String,
    challenge: String,
    w: Option<String>,
    session_id: Option<String>,
    proxy: Option<String>,
}

#[derive(Deserialize)]
struct GetTypeRequest {
    gt: String,
    challenge: String,
    w: Option<String>,
    session_id: Option<String>,
    proxy: Option<String>,
}

#[derive(Deserialize)]
struct VerifyRequest {
    gt: String,
    challenge: String,
    w: Option<String>,
    session_id: Option<String>,
    proxy: Option<String>,
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
}

#[derive(Deserialize)]
struct TestRequest {
    url: String,
    session_id: Option<String>,
    proxy: Option<String>,
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

// --- 辅助函数：获取实例，释放锁 ---
// 修复：返回 Json 类型
fn get_click_instance(state: &AppState, session_id: Option<String>, proxy: Option<String>) -> Result<Click, Json<ApiResponse<()>>> {
    let session_id = session_id.unwrap_or_else(|| "default".to_string());
    let mut instances = state.click_instances.lock().unwrap();

    if let Some(proxy_url) = proxy {
        match Click::new_with_proxy(&proxy_url) {
            Ok(new_instance) => {
                instances.insert(session_id.clone(), new_instance);
            }
            Err(e) => return Err(Json(ApiResponse::error(format!("设置代理失败: {}", e)))),
        }
    }
    
    Ok(instances.entry(session_id).or_insert_with(Click::default).clone())
}

fn get_slide_instance(state: &AppState, session_id: Option<String>, proxy: Option<String>) -> Result<Slide, Json<ApiResponse<()>>> {
    let session_id = session_id.unwrap_or_else(|| "default".to_string());
    let mut instances = state.slide_instances.lock().unwrap();

    if let Some(proxy_url) = proxy {
        match Slide::new_with_proxy(&proxy_url) {
            Ok(new_instance) => {
                instances.insert(session_id.clone(), new_instance);
            }
            Err(e) => return Err(Json(ApiResponse::error(format!("设置代理失败: {}", e)))),
        }
    }
    
    Ok(instances.entry(session_id).or_insert_with(Slide::default).clone())
}

// 辅助宏来简化 handler 中的错误处理
macro_rules! handle_blocking_call {
    ($instance_result:expr, $block:expr, $err_type:ty) => {
        {
            let mut instance = match $instance_result {
                Ok(inst) => inst,
                Err(Json(e)) => return Json(ApiResponse::<$err_type>::error(e.error.unwrap_or_default())),
            };

            let res = task::spawn_blocking(move || $block(&mut instance)).await.unwrap();

            match res {
                Ok(data) => Json(ApiResponse::success(data)),
                Err(e) => Json(ApiResponse::<$err_type>::error(e.to_string())),
            }
        }
    };
}


// --- Click 相关的处理函数 (使用宏简化) ---
async fn click_simple_match(State(state): State<AppState>, Json(req): Json<SimpleMatchRequest>) -> Json<ApiResponse<String>> {
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy),
        move |instance: &mut Click| instance.simple_match(&req.gt, &req.challenge),
        String
    )
}

async fn click_simple_match_retry(State(state): State<AppState>, Json(req): Json<SimpleMatchRequest>) -> Json<ApiResponse<String>> {
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy),
        move |instance: &mut Click| instance.simple_match_retry(&req.gt, &req.challenge),
        String
    )
}

async fn click_register_test(State(state): State<AppState>, Json(req): Json<RegisterTestRequest>) -> Json<ApiResponse<TupleResponse2>> {
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy),
        move |instance: &mut Click| instance.register_test(&req.url).map(|(f, s)| TupleResponse2 { first: f, second: s }),
        TupleResponse2
    )
}

async fn click_get_c_s(State(state): State<AppState>, Json(req): Json<GetCSRequest>) -> Json<ApiResponse<CSResponse>> {
    let w_owned = req.w.clone();
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy),
        move |instance: &mut Click| instance.get_c_s(&req.gt, &req.challenge, w_owned.as_deref()).map(|(c, s)| CSResponse { c, s }),
        CSResponse
    )
}

async fn click_get_type(State(state): State<AppState>, Json(req): Json<GetTypeRequest>) -> Json<ApiResponse<String>> {
    let w_owned = req.w.clone();
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy),
        move |instance: &mut Click| instance.get_type(&req.gt, &req.challenge, w_owned.as_deref()).map(|t| match t {
            VerifyType::Click => "click".to_string(),
            VerifyType::Slide => "slide".to_string(),
        }),
        String
    )
}

async fn click_verify(State(state): State<AppState>, Json(req): Json<VerifyRequest>) -> Json<ApiResponse<TupleResponse2>> {
    let w_owned = req.w.clone();
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy),
        move |instance: &mut Click| instance.verify(&req.gt, &req.challenge, w_owned.as_deref()).map(|(f, s)| TupleResponse2 { first: f, second: s }),
        TupleResponse2
    )
}

async fn click_generate_w(State(state): State<AppState>, Json(req): Json<GenerateWRequest>) -> Json<ApiResponse<String>> {
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy),
        move |instance: &mut Click| instance.generate_w(&req.key, &req.gt, &req.challenge, &req.c, &req.s),
        String
    )
}

async fn click_test(State(state): State<AppState>, Json(req): Json<TestRequest>) -> Json<ApiResponse<String>> {
    handle_blocking_call!(
        get_click_instance(&state, req.session_id, req.proxy),
        move |instance: &mut Click| instance.test(&req.url),
        String
    )
}

// --- Slide 相关的处理函数 (使用宏简化) ---
async fn slide_register_test(State(state): State<AppState>, Json(req): Json<RegisterTestRequest>) -> Json<ApiResponse<TupleResponse2>> {
    handle_blocking_call!(
        get_slide_instance(&state, req.session_id, req.proxy),
        move |instance: &mut Slide| instance.register_test(&req.url).map(|(f, s)| TupleResponse2 { first: f, second: s }),
        TupleResponse2
    )
}

async fn slide_get_c_s(State(state): State<AppState>, Json(req): Json<GetCSRequest>) -> Json<ApiResponse<CSResponse>> {
    let w_owned = req.w.clone();
    handle_blocking_call!(
        get_slide_instance(&state, req.session_id, req.proxy),
        move |instance: &mut Slide| instance.get_c_s(&req.gt, &req.challenge, w_owned.as_deref()).map(|(c, s)| CSResponse { c, s }),
        CSResponse
    )
}

async fn slide_get_type(State(state): State<AppState>, Json(req): Json<GetTypeRequest>) -> Json<ApiResponse<String>> {
    let w_owned = req.w.clone();
    handle_blocking_call!(
        get_slide_instance(&state, req.session_id, req.proxy),
        move |instance: &mut Slide| instance.get_type(&req.gt, &req.challenge, w_owned.as_deref()).map(|t| match t {
            VerifyType::Click => "click".to_string(),
            VerifyType::Slide => "slide".to_string(),
        }),
        String
    )
}

async fn slide_verify(State(state): State<AppState>, Json(req): Json<VerifyRequest>) -> Json<ApiResponse<TupleResponse2>> {
    let w_owned = req.w.clone();
    handle_blocking_call!(
        get_slide_instance(&state, req.session_id, req.proxy),
        move |instance: &mut Slide| instance.verify(&req.gt, &req.challenge, w_owned.as_deref()).map(|(f, s)| TupleResponse2 { first: f, second: s }),
        TupleResponse2
    )
}

async fn slide_generate_w(State(state): State<AppState>, Json(req): Json<GenerateWRequest>) -> Json<ApiResponse<String>> {
    handle_blocking_call!(
        get_slide_instance(&state, req.session_id, req.proxy),
        move |instance: &mut Slide| instance.generate_w(&req.key, &req.gt, &req.challenge, &req.c, &req.s),
        String
    )
}

async fn slide_test(State(state): State<AppState>, Json(req): Json<TestRequest>) -> Json<ApiResponse<String>> {
    handle_blocking_call!(
        get_slide_instance(&state, req.session_id, req.proxy),
        move |instance: &mut Slide| instance.test(&req.url),
        String
    )
}


// 健康检查端点
async fn health_check() -> &'static str {
    "OK"
}

#[tokio::main]
async fn main() {
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
        .layer(ServiceBuilder::new().layer(CorsLayer::permissive()))
        .with_state(state);

    let listener = TcpListener::bind("0.0.0.0:3000").await.unwrap();
        
    println!("🚀 Server starting on http://0.0.0.0:3000");
    println!("📋 Available endpoints:");
    println!("  GET  /health - Health check");
    println!("  POST /click/* - All click operations");
    println!("  POST /slide/* - All slide operations");
    println!("  (All POST endpoints accept optional 'proxy' and 'session_id' fields)");
    
    axum::serve(listener, app).await.unwrap();
}