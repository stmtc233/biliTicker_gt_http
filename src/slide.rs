// slide.rs

use crate::abstraction::{Api, GenerateW, Test, VerifyType};
use crate::error::{
    missing_param, net_work_error, other, other_without_source, parse_error, Result,
};
use captcha_breaker::captcha::Slide0;
use image::{DynamicImage, GenericImage};
use reqwest::blocking::Client;
use serde_json::Value;
use std::collections::HashMap;
use crate::w::slide_calculate;

#[derive(Clone)]
pub struct Slide {
    client: Client,
    noproxy_client: Client,
    verify_type: VerifyType,
}

impl Default for Slide {
    fn default() -> Self {
        Slide {
            client: Client::new(),
            noproxy_client: Client::new(),
            verify_type: VerifyType::Slide,
        }
    }
}

impl Slide {
    pub fn new_with_proxy(proxy_url: &str) -> Result<Self> {
        // 修复：使用 Proxy::all
        let proxy = reqwest::Proxy::all(proxy_url)
            .map_err(|e| other("无效的代理 URL", e))?;
        let proxied_client = Client::builder()
            .proxy(proxy)
            .build()
            .map_err(|e| other("构建代理客户端失败", e))?;
        
        Ok(Slide {
            client: proxied_client,
            noproxy_client: Client::new(),
            verify_type: VerifyType::Slide,
        })
    }
}

impl Api for Slide {
    type ArgsType = (String, String, String, String);

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
        let c: Vec<u8> = serde_json::from_value(res.get("c").ok_or_else(|| missing_param("c"))?.clone())
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
            res.get("s").expect("没有s").as_str().unwrap().to_string(),
            (
                res.get("challenge")
                    .ok_or_else(|| missing_param("challenge"))?
                    .as_str()
                    .ok_or_else(|| missing_param("challenge"))?
                    .to_string(),
                format!("https://{}{}", static_server, res.get("fullbg").ok_or_else(|| missing_param("fullbg"))?.as_str().ok_or_else(|| missing_param("fullbg"))?),
                format!("https://{}{}", static_server, res.get("bg").ok_or_else(|| missing_param("bg"))?.as_str().ok_or_else(|| missing_param("bg"))?),
                format!("https://{}{}", static_server, res.get("slice").ok_or_else(|| missing_param("slice"))?.as_str().ok_or_else(|| missing_param("slice"))?),
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
        let res = self.client().get(url).query(&params).send().map_err(net_work_error)?;
        let res = res.text().unwrap();
        let res = res
            .strip_prefix("geetest_1717918222610(")
            .ok_or_else(|| other_without_source("前缀错误"))?
            .strip_suffix(")")
            .ok_or_else(|| other_without_source("后缀错误"))?;
        let res: Value = serde_json::from_str(res).unwrap();
        Ok((
            res.get("message").ok_or_else(|| missing_param("message"))?.as_str().ok_or_else(|| missing_param("message"))?.to_string(),
            res.get("validate").ok_or_else(|| missing_param("validate"))?.as_str().ok_or_else(|| missing_param("validate"))?.to_string(),
        ))
    }

    fn refresh(&self, _gt: &str, _challenge: &str) -> Result<Self::ArgsType> {
        todo!("{}", "暂时不写")
    }

    fn client(&self) -> &Client { &self.client }
    fn noproxy_client(&self) -> &Client { &self.noproxy_client }
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
        let (w_sep, h_sep) = (10u32, 80u32); // Use u32 for consistency
        for idx in 0..52 {
            let x = (offset[idx] % 26 * 12) as u32;
            let y = if offset[idx] > 25 { h_sep } else { 0 };
            let new_x = (idx % 26 * 10) as u32; // 修复: usize -> u32
            let new_y = if idx > 25 { h_sep } else { 0 }; // 修复: usize -> u32
            
            let pi = bg_img.crop_imm(x, y, w_sep, h_sep);
            new_bg_img.copy_from(&pi, new_x, new_y).unwrap();
        }
        let new_bg_img = DynamicImage::ImageRgba8(new_bg_img);
        let res_x = Slide0::run(&slice_img, &new_bg_img).map_err(|_| other_without_source("滑块识别内部错误"))?.x1;
        Ok(res_x.to_string())
    }

    fn generate_w(&self, key: &str, gt: &str, challenge: &str, c: &[u8], s: &str) -> Result<String> {
        Ok(slide_calculate(key.parse().map_err(|e| other("滑动距离不是整数类型", e))?, gt, challenge, c, s))
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
        let w = slide_calculate(key.parse().unwrap(), &gt, &challenge, &c, &s);
        let (_, validate) = self.verify(gt.as_str(), challenge.as_str(), Some(w.as_str()))?;
        Ok(validate)
    }
}