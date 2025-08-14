// click.rs

use crate::abstraction::{Api, GenerateW, Test, VerifyType};
use crate::error::{
    missing_param, net_work_error, other, other_without_source, parse_error, Result,
};
use captcha_breaker::captcha::{Captcha, ChineseClick0}; // 引入 Captcha trait
use captcha_breaker::environment::CaptchaEnvironment;    // 引入 CaptchaEnvironment
use once_cell::sync::Lazy; // 引入 Lazy
use reqwest::blocking::Client;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, Instant};
use crate::w::click_calculate;

// --- 全局模型实例 ---
// 使用 Lazy 来确保模型只在第一次被访问时加载一次，并且是线程安全的。
static GLOBAL_CLICK_BREAKER: Lazy<Arc<ChineseClick0>> = Lazy::new(|| {
    println!("Loading ChineseClick0 ONNX model... This should only happen once.");
    let env = CaptchaEnvironment::default();
    let breaker = env.load_captcha_breaker::<ChineseClick0>().unwrap();
    println!("Model loaded successfully.");
    Arc::new(breaker)
});


#[derive(Clone)]
pub struct Click {
    client: Client,
    noproxy_client: Client,
    verify_type: VerifyType,
    cb: Arc<ChineseClick0>, // 这个字段保持不变，它将持有对全局实例的引用
}

impl Default for Click {
    fn default() -> Self {
        Click {
            client: Client::new(),
            noproxy_client: Client::new(),
            verify_type: VerifyType::Click,
            // 不再重新加载，而是克隆全局实例的 Arc 指针
            cb: Arc::clone(&GLOBAL_CLICK_BREAKER),
        }
    }
}

impl Click {
    pub fn new_with_proxy(proxy_url: &str) -> Result<Self> {
        let proxy = reqwest::Proxy::all(proxy_url)
            .map_err(|e| other("无效的代理 URL", e))?;
        let proxied_client = Client::builder()
            .proxy(proxy)
            .build()
            .map_err(|e| other("构建代理客户端失败", e))?;
        
        Ok(Click {
            client: proxied_client,
            noproxy_client: Client::new(),
            verify_type: VerifyType::Click,
            // 同样，克隆全局实例的 Arc 指针
            cb: Arc::clone(&GLOBAL_CLICK_BREAKER),
        })
    }
    
    // ... simple_match, simple_match_retry, vvv 等其他方法保持不变 ...
    pub fn simple_match(&mut self, gt: &str, challenge: &str) -> Result<String> {
        self.get_c_s(gt, challenge, None)?;
        self.get_type(gt, challenge, None)?;
        let (c, s, args) = self.get_new_c_s_args(gt, challenge)?;
        let start = Instant::now();
        let key = self.calculate_key(args)?;
        let w = self.generate_w(key.as_str(), gt, challenge, c.as_ref(), s.as_str())?;

        let elapsed = start.elapsed();
        if elapsed < Duration::from_secs(2) {
            let sleep_duration = Duration::from_secs(2) - elapsed;
            sleep(sleep_duration);
        }
        let (_, validate) = self.verify(gt, challenge, Some(w.as_str()))?;
        Ok(validate)
    }

    pub fn simple_match_retry(&mut self, gt: &str, challenge: &str) -> Result<String> {
        self.get_c_s(gt, challenge, None)?;
        self.get_type(gt, challenge, None)?;
        let (c, s, args) = self.get_new_c_s_args(gt, challenge)?;

        if let Ok(result) = self.vvv(gt, challenge, &c, s.as_str(), args) {
            return Ok(result);
        }

        loop {
            let args = self.refresh(gt, challenge)?;
            if let Ok(result) = self.vvv(gt, challenge, &c, s.as_str(), args) {
                return Ok(result);
            }
        }
    }

    fn vvv(&mut self, gt: &str, challenge: &str, c: &Vec<u8>, s: &str, args: String) -> Result<String> {
        let start = Instant::now();
        let key = self.calculate_key(args)?;
        let w = self.generate_w(key.as_str(), gt, challenge, c.as_ref(), s)?;
        
        let elapsed = start.elapsed();
        if elapsed < Duration::from_secs(2) {
            let sleep_duration = Duration::from_secs(2) - elapsed;
            sleep(sleep_duration);
        }

        let (_, validate) = self.verify(gt, challenge, Some(w.as_str()))?;
        Ok(validate)
    }
}


// Api, GenerateW, Test trait 的实现保持不变
// ...
impl Api for Click {
    type ArgsType = String;

    fn register_test(&self, url: &str) -> crate::error::Result<(String, String)> {
        let res = self.client().get(url).send().map_err(net_work_error)?;
        let res = res.json::<Value>().expect("解析失败");
        let res_data = res
            .get("data")
            .ok_or_else(|| missing_param("data"))?
            .get("geetest")
            .ok_or_else(|| missing_param("geetest"))?;
        Ok((
            res_data.get("gt")
                .ok_or_else(|| missing_param("gt"))?
                .as_str()
                .ok_or_else(|| missing_param("gt"))?
                .to_string(),
            res_data.get("challenge")
                .ok_or_else(|| missing_param("challenge"))?
                .as_str()
                .ok_or_else(|| missing_param("challenge"))?
                .to_string(),
        ))
    }

    fn get_new_c_s_args(&self, gt: &str, challenge: &str) -> Result<(Vec<u8>, String, Self::ArgsType)> {
        let url = "http://api.geevisit.com/get.php";
        let mut params = HashMap::from([
            ("gt", gt),
            ("challenge", challenge),
            ("is_next", "true"),
            ("offline", "false"),
            ("isPC", "true"),
            ("callback", "geetest_1717915671544"),
        ]);
        params.insert(
            "type",
            match self.verify_type {
                VerifyType::Click => "click",
                VerifyType::Slide => "slide",
            },
        );
        let res = self.client.get(url).query(&params).send().map_err(net_work_error)?;
        let res = res.text().map_err(|e| other("什么b玩意错误", e))?;
        let res = res
            .strip_prefix("geetest_1717915671544(")
            .ok_or_else(|| other_without_source("前缀错误"))?
            .strip_suffix(")")
            .ok_or_else(|| other_without_source("后缀错误"))?;
        let res: Value = serde_json::from_str(res).map_err(parse_error)?;
        let res_data = res.get("data").ok_or_else(|| missing_param("data"))?;
        let c: Vec<u8> = serde_json::from_value(res_data.get("c").ok_or_else(|| missing_param("c"))?.clone())
            .map_err(parse_error)?;
        let static_server = res_data
            .get("static_servers")
            .ok_or_else(|| missing_param("static_servers"))?
            .as_array()
            .ok_or_else(|| missing_param("static_servers"))?
            .get(0)
            .ok_or_else(|| other_without_source("static_servers里面咋没东西啊"))?
            .as_str()
            .ok_or_else(|| other_without_source("static_servers里面咋没东西啊"))?;
        Ok((
            c,
            res_data.get("s")
                .ok_or_else(|| missing_param("s"))?
                .as_str()
                .ok_or_else(|| missing_param("s"))?
                .to_string(),
            format!(
                "https://{}{}",
                static_server,
                res_data.get("pic")
                    .ok_or_else(|| missing_param("pic"))?
                    .as_str()
                    .ok_or_else(|| missing_param("pic"))?
                    .strip_prefix("/")
                    .ok_or_else(|| other_without_source("我真不想编错误名了"))?
            ),
        ))
    }

    fn verify(&self, gt: &str, challenge: &str, w: Option<&str>) -> Result<(String, String)> {
        let url = "http://api.geevisit.com/ajax.php";
        let mut params = HashMap::from([
            ("gt", gt),
            ("challenge", challenge),
            ("callback", "geetest_1717918222610"),
        ]);
        if let Some(w) = w {
            params.insert("w", w);
        }
        let res = self.client.get(url).query(&params).send().map_err(net_work_error)?;
        let res = res.text().map_err(|e| other("什么b玩意错误", e))?;
        let res = res
            .strip_prefix("geetest_1717918222610(")
            .ok_or_else(|| other_without_source("前缀错误"))?
            .strip_suffix(")")
            .ok_or_else(|| other_without_source("后缀错误"))?;
        let res: Value = serde_json::from_str(res).map_err(parse_error)?;
        let res_data = res.get("data").ok_or_else(|| missing_param("data"))?;
        Ok((
            res_data.get("result")
                .ok_or_else(|| missing_param("result"))?
                .as_str()
                .ok_or_else(|| missing_param("result"))?
                .to_string(),
            res_data.get("validate")
                .ok_or_else(|| missing_param("validate"))?
                .as_str()
                .ok_or_else(|| missing_param("validate"))?
                .to_string(),
        ))
    }

    fn refresh(&self, gt: &str, challenge: &str) -> Result<Self::ArgsType> {
        let url = "http://api.geevisit.com/refresh.php";
        let params = HashMap::from([
            ("gt", gt),
            ("challenge", challenge),
            ("callback", "geetest_1717918222610"),
        ]);
        let res = self.client.get(url).query(&params).send().map_err(net_work_error)?;
        let res = res.text().map_err(|e| other("什么b玩意错误", e))?;
        let res = res
            .strip_prefix("geetest_1717918222610(")
            .ok_or_else(|| other_without_source("前缀错误"))?
            .strip_suffix(")")
            .ok_or_else(|| other_without_source("后缀错误"))?;
        let res: Value = serde_json::from_str(res).map_err(parse_error)?;
        let res_data = res.get("data").ok_or_else(|| missing_param("data"))?;
        let static_server = res_data
            .get("image_servers")
            .ok_or_else(|| missing_param("image_servers"))?
            .as_array()
            .ok_or_else(|| missing_param("image_servers"))?
            .get(0)
            .ok_or_else(|| other_without_source("image_servers里面咋没东西啊"))?
            .as_str()
            .ok_or_else(|| other_without_source("image_servers里面咋没东西啊"))?;
        Ok(format!(
            "https://{}{}",
            static_server,
            res_data.get("pic")
                .ok_or_else(|| missing_param("pic"))?
                .as_str()
                .ok_or_else(|| missing_param("pic"))?
                .strip_prefix("/")
                .ok_or_else(|| other_without_source("我真不想编错误名了"))?
        ))
    }

    fn client(&self) -> &Client { &self.client }
    fn noproxy_client(&self) -> &Client { &self.noproxy_client }
}

impl GenerateW for Click {
    fn calculate_key(&mut self, args: Self::ArgsType) -> Result<String> {
        let pic_url = args;
        let pic_img = self.download_img(pic_url.as_str())?;
        let pic_img = image::load_from_memory(&pic_img).map_err(|e| other("图片加载失败", e))?;

        let cb_res = self.cb.run(&pic_img).map_err(|_| other_without_source("cb模块内部错误"))?;
        let mut res = vec![];
        for (x, y) in &cb_res {
            let position = format!(
                "{}_{}",
                (x / 333.375 * 100f32 * 100f32).round(),
                (y / 333.375 * 100f32 * 100f32).round()
            );
            res.push(position);
        }
        Ok(res.join(","))
    }

    fn generate_w(&self, key: &str, gt: &str, challenge: &str, _c: &[u8], _s: &str) -> Result<String> {
        Ok(click_calculate(key, gt, challenge))
    }
}

impl Test for Click {
    fn test(&mut self, url: &str) -> Result<String> {
        let (gt, challenge) = self.register_test(url)?;
        let (c, s) = self.get_c_s(gt.as_str(), challenge.as_str(), None)?;
        self.get_type(gt.as_str(), challenge.as_str(), None)?;
        let (_c, _s, args) = self.get_new_c_s_args(gt.as_str(), challenge.as_str())?;
        let key = self.calculate_key(args)?;
        let w = self.generate_w(key.as_str(), gt.as_str(), challenge.as_str(), c.as_ref(), s.as_str())?;

        sleep(Duration::new(2, 0));
        let (_, validate) = self.verify(gt.as_str(), challenge.as_str(), Some(w.as_str()))?;
        Ok(validate)
    }
}