use crate::translation::{Translation, Output};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use anyhow::{Result, bail};
use rand::Rng;
use rand::rngs::ThreadRng;
use md5::{Digest, Md5};

#[derive(Debug)]
pub struct Baidu<'a> {
    pub word: &'a str,
    pub client: &'a Client,
    pub appid: &'a str,
    pub key: &'a str,
}

#[derive(Deserialize, Debug)]
struct BaiduTransResult {
    src: String,
    dst: String,
}

#[derive(Deserialize, Debug)]
struct BaiduRespSuccess {
    from: String,
    to: String,
    trans_result: Vec<BaiduTransResult>,
}

#[derive(Deserialize, Debug)]
struct BaiduRespError {
    error_code: String,
    error_msg: String,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum BaiduResponse {
    Success(BaiduRespSuccess),
    Error(BaiduRespError),
}

impl<'a> Translation for Baidu<'a> {
    fn translate(&self) -> Result<Output> {
        const URL_BAIDU: &str = "https://fanyi-api.baidu.com/api/trans/vip/translate";
        let mut rng = ThreadRng::default();
        let salt: u32 = rng.random();
        let salt = salt.to_string();
        let sign = Md5::digest(format!("{}{}{}{}", self.appid, self.word, salt, self.key));
        let sign = format!("{:x}", sign);

        let params = [
            ("q", self.word),
            ("from", "auto"),
            ("to", "zh"),
            ("appid", self.appid),
            ("salt", &salt),
            ("sign", &sign),
        ];

        let resp = self
            .client
            .post(URL_BAIDU)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&params)
            .send()?;

        if !resp.status().is_success() {
            bail!("HTTP error: {}", resp.status());
        }

        let resp = match resp.text() {
            Ok(t) => t,
            Err(e) => {
                bail!("HTTP response error: {:?}", e);
            }
        };

        let mut output = Output::new(self.word);
        let resp = serde_json::from_str::<BaiduResponse>(resp.as_str())?;
        match resp {
            BaiduResponse::Success(s) => {
                let mut meanings: Vec<String> = Vec::new();
                for r in s.trans_result {
                    meanings.push(r.dst);
                }
                output.meanings = Some(meanings);
            }
            BaiduResponse::Error(e) => {
                bail!("Response error: {:?}", e);
            }
        }
        Ok(output)
    }
}
