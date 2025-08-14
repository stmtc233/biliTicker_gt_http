use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;

// 引入你的模块
mod abstraction;
mod click;
mod error;
mod slide;
mod w; // 添加缺失的 w 模块

use crate::abstraction::{Api, GenerateW, Test, VerifyType};
use crate::click::Click;
use crate::slide::Slide;

// 应用状态，用于存储 Click 和 Slide 实例
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

// 请求和响应结构体
#[derive(Deserialize)]
struct SimpleMatchRequest {
    gt: String,
    challenge: String,
    session_id: Option<String>,
}

#[derive(Deserialize)]
struct RegisterTestRequest {
    url: String,
    session_id: Option<String>,
}

#[derive(Deserialize)]
struct GetCSRequest {
    gt: String,
    challenge: String,
    w: Option<String>,
    session_id: Option<String>,
}

#[derive(Deserialize)]
struct GetTypeRequest {
    gt: String,
    challenge: String,
    w: Option<String>,
    session_id: Option<String>,
}

#[derive(Deserialize)]
struct VerifyRequest {
    gt: String,
    challenge: String,
    w: Option<String>,
    session_id: Option<String>,
}

#[derive(Deserialize)]
struct GenerateWRequest {
    key: String,
    gt: String,
    challenge: String,
    c: Vec<u8>,
    s: String,
    session_id: Option<String>,
}

#[derive(Deserialize)]
struct TestRequest {
    url: String,
    session_id: Option<String>,
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

// Click 相关的处理函数
async fn click_simple_match(
    State(state): State<AppState>,
    Json(req): Json<SimpleMatchRequest>,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    let session_id = req.session_id.unwrap_or_else(|| "default".to_string());
    
    let mut instances = state.click_instances.lock().unwrap();
    let click = instances.entry(session_id).or_insert_with(Click::default);
    
    match click.simple_match(&req.gt, &req.challenge) {
        Ok(validate) => Ok(Json(ApiResponse::success(validate))),
        Err(e) => Ok(Json(ApiResponse::error(e.to_string()))),
    }
}

async fn click_simple_match_retry(
    State(state): State<AppState>,
    Json(req): Json<SimpleMatchRequest>,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    let session_id = req.session_id.unwrap_or_else(|| "default".to_string());
    
    let mut instances = state.click_instances.lock().unwrap();
    let click = instances.entry(session_id).or_insert_with(Click::default);
    
    match click.simple_match_retry(&req.gt, &req.challenge) {
        Ok(validate) => Ok(Json(ApiResponse::success(validate))),
        Err(e) => Ok(Json(ApiResponse::error(e.to_string()))),
    }
}

async fn click_register_test(
    State(state): State<AppState>,
    Json(req): Json<RegisterTestRequest>,
) -> Result<Json<ApiResponse<TupleResponse2>>, StatusCode> {
    let session_id = req.session_id.unwrap_or_else(|| "default".to_string());
    
    let mut instances = state.click_instances.lock().unwrap();
    let click = instances.entry(session_id).or_insert_with(Click::default);
    
    match click.register_test(&req.url) {
        Ok((first, second)) => Ok(Json(ApiResponse::success(TupleResponse2 { first, second }))),
        Err(e) => Ok(Json(ApiResponse::error(e.to_string()))),
    }
}

async fn click_get_c_s(
    State(state): State<AppState>,
    Json(req): Json<GetCSRequest>,
) -> Result<Json<ApiResponse<CSResponse>>, StatusCode> {
    let session_id = req.session_id.unwrap_or_else(|| "default".to_string());
    
    let mut instances = state.click_instances.lock().unwrap();
    let click = instances.entry(session_id).or_insert_with(Click::default);
    
    match click.get_c_s(&req.gt, &req.challenge, req.w.as_deref()) {
        Ok((c, s)) => Ok(Json(ApiResponse::success(CSResponse { c, s }))),
        Err(e) => Ok(Json(ApiResponse::error(e.to_string()))),
    }
}

async fn click_get_type(
    State(state): State<AppState>,
    Json(req): Json<GetTypeRequest>,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    let session_id = req.session_id.unwrap_or_else(|| "default".to_string());
    
    let instances = state.click_instances.lock().unwrap();
    let default_click = Click::default();
    let click = instances.get(&session_id).unwrap_or(&default_click);
    
    match click.get_type(&req.gt, &req.challenge, req.w.as_deref()) {
        Ok(verify_type) => {
            let type_str = match verify_type {
                VerifyType::Slide => "slide".to_string(),
                VerifyType::Click => "click".to_string(),
            };
            Ok(Json(ApiResponse::success(type_str)))
        }
        Err(e) => Ok(Json(ApiResponse::error(e.to_string()))),
    }
}

async fn click_verify(
    State(state): State<AppState>,
    Json(req): Json<VerifyRequest>,
) -> Result<Json<ApiResponse<TupleResponse2>>, StatusCode> {
    let session_id = req.session_id.unwrap_or_else(|| "default".to_string());
    
    let instances = state.click_instances.lock().unwrap();
    let default_click = Click::default();
    let click = instances.get(&session_id).unwrap_or(&default_click);
    
    match click.verify(&req.gt, &req.challenge, req.w.as_deref()) {
        Ok((first, second)) => Ok(Json(ApiResponse::success(TupleResponse2 { first, second }))),
        Err(e) => Ok(Json(ApiResponse::error(e.to_string()))),
    }
}

async fn click_generate_w(
    State(state): State<AppState>,
    Json(req): Json<GenerateWRequest>,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    let session_id = req.session_id.unwrap_or_else(|| "default".to_string());
    
    let instances = state.click_instances.lock().unwrap();
    let default_click = Click::default();
    let click = instances.get(&session_id).unwrap_or(&default_click);
    
    match click.generate_w(&req.key, &req.gt, &req.challenge, &req.c, &req.s) {
        Ok(w) => Ok(Json(ApiResponse::success(w))),
        Err(e) => Ok(Json(ApiResponse::error(e.to_string()))),
    }
}

async fn click_test(
    State(state): State<AppState>,
    Json(req): Json<TestRequest>,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    let session_id = req.session_id.unwrap_or_else(|| "default".to_string());
    
    let mut instances = state.click_instances.lock().unwrap();
    let click = instances.entry(session_id).or_insert_with(Click::default);
    
    match click.test(&req.url) {
        Ok(result) => Ok(Json(ApiResponse::success(result))),
        Err(e) => Ok(Json(ApiResponse::error(e.to_string()))),
    }
}

// Slide 相关的处理函数
async fn slide_register_test(
    State(state): State<AppState>,
    Json(req): Json<RegisterTestRequest>,
) -> Result<Json<ApiResponse<TupleResponse2>>, StatusCode> {
    let session_id = req.session_id.unwrap_or_else(|| "default".to_string());
    
    let mut instances = state.slide_instances.lock().unwrap();
    let slide = instances.entry(session_id).or_insert_with(Slide::default);
    
    match slide.register_test(&req.url) {
        Ok((first, second)) => Ok(Json(ApiResponse::success(TupleResponse2 { first, second }))),
        Err(e) => Ok(Json(ApiResponse::error(e.to_string()))),
    }
}

async fn slide_get_c_s(
    State(state): State<AppState>,
    Json(req): Json<GetCSRequest>,
) -> Result<Json<ApiResponse<CSResponse>>, StatusCode> {
    let session_id = req.session_id.unwrap_or_else(|| "default".to_string());
    
    let mut instances = state.slide_instances.lock().unwrap();
    let slide = instances.entry(session_id).or_insert_with(Slide::default);
    
    match slide.get_c_s(&req.gt, &req.challenge, req.w.as_deref()) {
        Ok((c, s)) => Ok(Json(ApiResponse::success(CSResponse { c, s }))),
        Err(e) => Ok(Json(ApiResponse::error(e.to_string()))),
    }
}

async fn slide_get_type(
    State(state): State<AppState>,
    Json(req): Json<GetTypeRequest>,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    let session_id = req.session_id.unwrap_or_else(|| "default".to_string());
    
    let instances = state.slide_instances.lock().unwrap();
    let default_slide = Slide::default();
    let slide = instances.get(&session_id).unwrap_or(&default_slide);
    
    match slide.get_type(&req.gt, &req.challenge, req.w.as_deref()) {
        Ok(verify_type) => {
            let type_str = match verify_type {
                VerifyType::Slide => "slide".to_string(),
                VerifyType::Click => "click".to_string(),
            };
            Ok(Json(ApiResponse::success(type_str)))
        }
        Err(e) => Ok(Json(ApiResponse::error(e.to_string()))),
    }
}

async fn slide_verify(
    State(state): State<AppState>,
    Json(req): Json<VerifyRequest>,
) -> Result<Json<ApiResponse<TupleResponse2>>, StatusCode> {
    let session_id = req.session_id.unwrap_or_else(|| "default".to_string());
    
    let instances = state.slide_instances.lock().unwrap();
    let default_slide = Slide::default();
    let slide = instances.get(&session_id).unwrap_or(&default_slide);
    
    match slide.verify(&req.gt, &req.challenge, req.w.as_deref()) {
        Ok((first, second)) => Ok(Json(ApiResponse::success(TupleResponse2 { first, second }))),
        Err(e) => Ok(Json(ApiResponse::error(e.to_string()))),
    }
}

async fn slide_generate_w(
    State(state): State<AppState>,
    Json(req): Json<GenerateWRequest>,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    let session_id = req.session_id.unwrap_or_else(|| "default".to_string());
    
    let instances = state.slide_instances.lock().unwrap();
    let default_slide = Slide::default();
    let slide = instances.get(&session_id).unwrap_or(&default_slide);
    
    match slide.generate_w(&req.key, &req.gt, &req.challenge, &req.c, &req.s) {
        Ok(w) => Ok(Json(ApiResponse::success(w))),
        Err(e) => Ok(Json(ApiResponse::error(e.to_string()))),
    }
}

async fn slide_test(
    State(state): State<AppState>,
    Json(req): Json<TestRequest>,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    let session_id = req.session_id.unwrap_or_else(|| "default".to_string());
    
    let mut instances = state.slide_instances.lock().unwrap();
    let slide = instances.entry(session_id).or_insert_with(Slide::default);
    
    match slide.test(&req.url) {
        Ok(result) => Ok(Json(ApiResponse::success(result))),
        Err(e) => Ok(Json(ApiResponse::error(e.to_string()))),
    }
}

// 健康检查端点
async fn health_check() -> &'static str {
    "OK"
}

#[tokio::main]
async fn main() {
    // 初始化状态
    let state = AppState::new();

    // 创建路由
    let app = Router::new()
        .route("/health", get(health_check))
        
        // Click 相关路由
        .route("/click/simple_match", post(click_simple_match))
        .route("/click/simple_match_retry", post(click_simple_match_retry))
        .route("/click/register_test", post(click_register_test))
        .route("/click/get_c_s", post(click_get_c_s))
        .route("/click/get_type", post(click_get_type))
        .route("/click/verify", post(click_verify))
        .route("/click/generate_w", post(click_generate_w))
        .route("/click/test", post(click_test))
        
        // Slide 相关路由
        .route("/slide/register_test", post(slide_register_test))
        .route("/slide/get_c_s", post(slide_get_c_s))
        .route("/slide/get_type", post(slide_get_type))
        .route("/slide/verify", post(slide_verify))
        .route("/slide/generate_w", post(slide_generate_w))
        .route("/slide/test", post(slide_test))
        
        .layer(
            ServiceBuilder::new()
                .layer(CorsLayer::permissive()) // 允许跨域请求
        )
        .with_state(state);

    // 启动服务器
    let listener = TcpListener::bind("0.0.0.0:3000")
        .await
        .unwrap();
        
    println!("🚀 Server starting on http://0.0.0.0:3000");
    println!("📋 Available endpoints:");
    println!("  GET  /health - Health check");
    println!("  POST /click/simple_match - Click simple match");
    println!("  POST /click/simple_match_retry - Click simple match with retry");
    println!("  POST /click/* - Other click operations");
    println!("  POST /slide/* - Slide operations");
    
    axum::serve(listener, app)
        .await
        .unwrap();
}