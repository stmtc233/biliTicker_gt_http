// slide.rs

use crate::abstraction::{Api, GenerateW, Test, VerifyType};
use crate::error::{
    missing_param, net_work_error, other, other_without_source, parse_error, Result,
};
use crate::w::slide_calculate;
use captcha_breaker::captcha::Slide0;
use image::{DynamicImage, GenericImage};
use reqwest::blocking::Client;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
// 修改：引入 SystemTime 和 UNIX_EPOCH 用于生成时间戳
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct Slide {
    client: Arc<Client>,
    noproxy_client: Arc<Client>,
    verify_type: VerifyType,
}

impl Slide {
    pub fn new(client: Arc<Client>, noproxy_client: Arc<Client>) -> Self {
        Slide {
            client,
            noproxy_client,
            verify_type: VerifyType::Slide,
        }
    }

    pub fn update_client(&mut self, new_client: Arc<Client>) {
        self.client = new_client;
    }
}

impl Api for Slide {
    type ArgsType = (String, String, String, String);

    fn client(&self) -> &Client {
        &self.client
    }
    fn noproxy_client(&self) -> &Client {
        &self.noproxy_client
    }

    fn get_new_c_s_args(
        &self,
        gt: &str,
        challenge: &str,
    ) -> Result<(Vec<u8>, String, Self::ArgsType)> {
        // 修改：生成动态回调
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_millis()
            .to_string();
        let callback = format!("geetest_{}", timestamp);

        let url = "http://api.geevisit.com/get.php";
        let mut params = HashMap::from([
            ("gt", gt),
            ("challenge", challenge),
            ("is_next", "true"),
            ("offline", "false"),
            ("isPC", "true"),
            ("callback", callback.as_str()), // 使用动态回调
        ]);
        params.insert(
            "type",
            match self.verify_type {
                VerifyType::Click => "click",
                VerifyType::Slide => "slide",
            },
        );
        let res = self
            .client
            .get(url)
            .query(&params)
            .send()
            .map_err(net_work_error)?;
        let res = res.text().map_err(|e| other("什么b玩意错误", e))?;

        // 修改：使用动态回调作为前缀
        let prefix = format!("{}(", callback);
        let res = res
            .strip_prefix(&prefix)
            .ok_or_else(|| other_without_source("前缀错误"))?
            .strip_suffix(")")
            .ok_or_else(|| other_without_source("后缀错误"))?;

        let res: Value = serde_json::from_str(res).map_err(parse_error)?;
        let c: Vec<u8> =
            serde_json::from_value(res.get("c").ok_or_else(|| missing_param("c"))?.clone())
                .map_err(parse_error)?;
        let static_server = res
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
            // 改进：使用安全的错误处理替换 expect 和 unwrap
            res.get("s")
                .ok_or_else(|| missing_param("s"))?
                .as_str()
                .ok_or_else(|| missing_param("s"))?
                .to_string(),
            (
                res.get("challenge")
                    .ok_or_else(|| missing_param("challenge"))?
                    .as_str()
                    .ok_or_else(|| missing_param("challenge"))?
                    .to_string(),
                format!(
                    "https://{}{}",
                    static_server,
                    res.get("fullbg")
                        .ok_or_else(|| missing_param("fullbg"))?
                        .as_str()
                        .ok_or_else(|| missing_param("fullbg"))?
                ),
                format!(
                    "https://{}{}",
                    static_server,
                    res.get("bg")
                        .ok_or_else(|| missing_param("bg"))?
                        .as_str()
                        .ok_or_else(|| missing_param("bg"))?
                ),
                format!(
                    "https://{}{}",
                    static_server,
                    res.get("slice")
                        .ok_or_else(|| missing_param("slice"))?
                        .as_str()
                        .ok_or_else(|| missing_param("slice"))?
                ),
            ),
        ))
    }

    fn verify(&self, gt: &str, challenge: &str, w: Option<&str>) -> Result<(String, String)> {
        // 修改：生成动态回调
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_millis()
            .to_string();
        let callback = format!("geetest_{}", timestamp);

        let url = "http://api.geevisit.com/ajax.php";
        let mut params = HashMap::from([
            ("gt", gt),
            ("challenge", challenge),
            ("callback", callback.as_str()), // 使用动态回调
        ]);
        if let Some(w) = w {
            params.insert("w", w);
        }
        let res = self
            .client()
            .get(url)
            .query(&params)
            .send()
            .map_err(net_work_error)?;
        // 改进：使用安全的错误处理替换 unwrap
        let res = res.text().map_err(|e| other("响应转文本失败", e))?;

        // 修改：使用动态回调作为前缀
        let prefix = format!("{}(", callback);
        let res = res
            .strip_prefix(&prefix)
            .ok_or_else(|| other_without_source("前缀错误"))?
            .strip_suffix(")")
            .ok_or_else(|| other_without_source("后缀错误"))?;

        // 改进：使用安全的错误处理替换 unwrap
        let res: Value = serde_json::from_str(res).map_err(parse_error)?;
        Ok((
            res.get("message")
                .ok_or_else(|| missing_param("message"))?
                .as_str()
                .ok_or_else(|| missing_param("message"))?
                .to_string(),
            res.get("validate")
                .ok_or_else(|| missing_param("validate"))?
                .as_str()
                .ok_or_else(|| missing_param("validate"))?
                .to_string(),
        ))
    }

    fn refresh(&self, _gt: &str, _challenge: &str) -> Result<Self::ArgsType> {
        todo!("{}", "暂时不写")
    }
}

impl GenerateW for Slide {
    fn calculate_key(&mut self, args: Self::ArgsType) -> Result<String> {
        let (_, _, bg, slice) = args;
        let bg_img = self.download_img(bg.as_str())?;
        let slice_img = self.download_img(slice.as_str())?;
        let slice_img = image::load_from_memory(&slice_img).map_err(|e| other("内部错误", e))?;
        let bg_img = image::load_from_memory(&bg_img).map_err(|e| other("图片解析错误", e))?;
        let mut new_bg_img = image::ImageBuffer::new(260, 160);
        let offset = [
            39, 38, 48, 49, 41, 40, 46, 47, 35, 34, 50, 51, 33, 32, 28, 29, 27, 26, 36, 37, 31, 30,
            44, 45, 43, 42, 12, 13, 23, 22, 14, 15, 21, 20, 8, 9, 25, 24, 6, 7, 3, 2, 0, 1, 11, 10,
            4, 5, 19, 18, 16, 17,
        ];
        let (w_sep, h_sep) = (10u32, 80u32);
        for idx in 0..52 {
            let x = (offset[idx] % 26 * 12) as u32;
            let y = if offset[idx] > 25 { h_sep } else { 0 };
            let new_x = (idx % 26 * 10) as u32;
            let new_y = if idx > 25 { h_sep } else { 0 };

            let pi = bg_img.crop_imm(x, y, w_sep, h_sep);
            new_bg_img.copy_from(&pi, new_x, new_y).unwrap();
        }
        let new_bg_img = DynamicImage::ImageRgba8(new_bg_img);
        let res_x = Slide0::run(&slice_img, &new_bg_img)
            .map_err(|_| other_without_source("滑块识别内部错误"))?
            .x1;
        Ok(res_x.to_string())
    }

    fn generate_w(&self, key: &str, gt: &str, challenge: &str, c: &[u8], s: &str) -> Result<String> {
        Ok(slide_calculate(
            key.parse()
                .map_err(|e| other("滑动距离不是整数类型", e))?,
            gt,
            challenge,
            c,
            s,
        ))
    }
}

impl Test for Slide {
    fn test(&mut self, url: &str) -> Result<String> {
        let (gt, mut challenge) = self.register_test(url)?;
        let (c, s) = self.get_c_s(gt.as_str(), challenge.as_str(), None)?;
        self.get_type(gt.as_str(), challenge.as_str(), None)?;
        let (_c, _s, args) = self.get_new_c_s_args(gt.as_str(), challenge.as_str())?;
        challenge = args.0.clone();
        let key = self.calculate_key(args)?;
        // 改进：使用 generate_w 方法以保持一致性，并进行错误处理
        let w = self.generate_w(key.as_str(), &gt, &challenge, &c, &s)?;
        let (_, validate) = self.verify(gt.as_str(), challenge.as_str(), Some(w.as_str()))?;
        Ok(validate)
    }
}