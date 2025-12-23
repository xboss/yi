use std::io::{self, Cursor, Read};
use std::time::Duration;

use clap::{Parser, command};
use md5::{Digest, Md5};
use owo_colors::{OwoColorize, colors::*};
use quick_xml::Reader;
use quick_xml::events::Event;
use rand::Rng;
use rand::rngs::ThreadRng;
use reqwest::blocking::Client;
use rodio::{Decoder, OutputStreamBuilder, Sink};
use serde::{Deserialize, Serialize};
use serde_json;
use std::env;

// iciba
#[derive(Debug)]
struct Iciba<'a> {
    word: &'a str,
    client: &'a Client,
}

impl<'a> Translation for Iciba<'a> {
    fn translate(&self) -> Result<Output, Box<dyn std::error::Error>> {
        const URL_ICIBA: &str = "https://dict-co.iciba.com/api/dictionary.php";
        let params = [
            ("key", "D191EBD014295E913574E1EAF8E06666"),
            ("w", &self.word),
        ];

        let resp = self.client.get(URL_ICIBA).query(&params).send();

        let resp = match resp {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                eprintln!("Error: request timed out.");
                return Err(Box::new(e));
            }
            Err(e) => {
                eprintln!("Network error: {:?}", e);
                return Err(Box::new(e));
            }
        };

        // check status code
        if !resp.status().is_success() {
            eprintln!("HTTP error: {}", resp.status());
            return Err(format!("HTTP status {}", resp.status()).into());
        }

        let resp = match resp.text() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("HTTP response error: {:?}", e);
                return Err(Box::new(e));
            }
        };

        let mut reader = Reader::from_str(&resp);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        let mut current_tag: Option<&'static str> = None;
        let mut key = String::new();
        let mut pos_list: Vec<String> = Vec::new();
        let mut acceptation_list: Vec<String> = Vec::new();
        let mut ps_list: Vec<String> = Vec::new();
        let mut pron_list: Vec<String> = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Err(e) => panic!("Error at postion {}: {:?}", reader.error_position(), e),
                Ok(Event::Eof) => break,
                Ok(Event::Start(e)) => match e.name().as_ref() {
                    b"key" => current_tag = Some("key"),
                    b"ps" => current_tag = Some("ps"),
                    b"pron" => current_tag = Some("pron"),
                    b"pos" => current_tag = Some("pos"),
                    b"acceptation" => current_tag = Some("acceptation"),
                    _ => (),
                },
                Ok(Event::End(_)) => {
                    current_tag = None;
                }
                Ok(Event::Text(e)) => {
                    let text = e.decode().unwrap().into_owned();
                    match current_tag {
                        Some("key") => key = text,
                        Some("pos") => pos_list.push(text),
                        Some("acceptation") => acceptation_list.push(text),
                        Some("ps") => ps_list.push(text),
                        Some("pron") => pron_list.push(text),
                        _ => (),
                    }
                }
                _ => (),
            }
            buf.clear();
        }

        let mut output = Output::new(self.word);
        match ps_list.len() {
            1 => output.phonetic_us = Some(ps_list[0].clone()),
            2 => {
                output.phonetic_uk = Some(ps_list[0].clone());
                output.phonetic_us = Some(ps_list[1].clone());
            }
            _ => (),
        }
        output.meanings = Some(acceptation_list);
        output.pos = Some(pos_list);
        Ok(output)
    }
}

// baidu
#[derive(Debug)]
struct Baidu<'a> {
    word: &'a str,
    client: &'a Client,
    appid: &'a str,
    key: &'a str,
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

// fn gen_baidu_sign(word: &str, appid: &str, key: &str) -> String {
//     let mut rng = ThreadRng::default();
//     let salt: u32 = rng.random();
//     let salt = salt.to_string();
//     let sign = Md5::digest(format!("{}{}{}{}", appid, word, salt, key));
//     let sign = format!("{:x}", sign);
//     return sign;
// }

impl<'a> Translation for Baidu<'a> {
    fn translate(&self) -> Result<Output, Box<dyn std::error::Error>> {
        const URL_BAIDU: &str = "https://fanyi-api.baidu.com/api/trans/vip/translate";
        // println!("baidu:{:?}", self);
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

        // println!("params: {:?}", params);

        let resp = self
            .client
            .post(URL_BAIDU)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&params)
            .send();

        let resp = match resp {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                eprintln!("Error: request timed out.");
                return Err(Box::new(e));
            }
            Err(e) => {
                eprintln!("Network error: {:?}", e);
                return Err(Box::new(e));
            }
        };

        if !resp.status().is_success() {
            eprintln!("HTTP error: {}", resp.status());
            return Err(format!("HTTP status {}", resp.status()).into());
        }

        let resp = match resp.text() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("HTTP response error: {:?}", e);
                return Err(Box::new(e));
            }
        };

        // println!("jsonstr:{:?}", resp);

        let mut output = Output::new(self.word);
        let resp = serde_json::from_str::<BaiduResponse>(resp.as_str())?;
        // println!("baiduresp:{:?}", resp);
        match resp {
            BaiduResponse::Success(s) => {
                let mut meanings: Vec<String> = Vec::new();
                for r in s.trans_result {
                    meanings.push(r.dst);
                }
                output.meanings = Some(meanings);
            }
            BaiduResponse::Error(e) => {
                eprintln!("Response error: {:?}", e);
                return Err(e.error_msg.into());
            }
        }
        Ok(output)
    }
}

// app

fn speak(
    word: &str,
    phonetic: Phonetic,
    client: &Client,
) -> Result<(), Box<dyn std::error::Error>> {
    let ps = if phonetic == Phonetic::Us {
        println!("美音朗读...");
        2
    } else {
        println!("英音朗读...");
        1
    };
    let url = format!(
        "https://dict.youdao.com/dictvoice?audio={}&type={}",
        word, ps
    );

    let resp = client.get(url).send();
    let resp = match resp {
        Ok(r) => r,
        Err(e) if e.is_timeout() => {
            eprintln!("Error: audio request timed out.");
            return Err(Box::new(e));
        }
        Err(e) => {
            eprintln!("Network error: {:?}", e);
            return Err(Box::new(e));
        }
    };

    // check status code
    if !resp.status().is_success() {
        eprintln!("HTTP error: {}", resp.status());
        return Err(format!("HTTP status {}", resp.status()).into());
    }

    let bytes = resp.bytes()?;
    let cursor = Cursor::new(bytes);
    let mut stream = OutputStreamBuilder::open_default_stream()?;
    stream.log_on_drop(false);
    let sink = Sink::connect_new(&stream.mixer());
    let source = Decoder::try_from(cursor)?;
    sink.append(source);
    sink.sleep_until_end();
    Ok(())
}

fn output_text(output: &Output, is_pure: bool) {
    println!("{}", output.word.fg::<Cyan>());
    if let Some(ps) = output.phonetic_uk.as_ref() {
        if is_pure {
            println!("英 /{}/ 美 /{}/", ps, output.phonetic_us.as_ref().unwrap());
        } else {
            println!(
                "英 /{}/ 美 /{}/",
                ps.green(),
                output.phonetic_us.as_ref().unwrap().green()
            );
        }
    } else {
        if let Some(ps) = output.phonetic_us.as_ref() {
            if is_pure {
                println!("/{}/", ps);
            } else {
                println!("/{}/", ps.green());
            }
        }
    }
    if let Some(meanings) = output.meanings.as_ref() {
        let mut i = 0;
        while i < meanings.len() {
            match output.pos.as_ref() {
                Some(pos) => {
                    if pos.len() > i {
                        print!("{} ", pos[i]);
                    }
                }
                None => {}
            }
            if is_pure {
                println!("{}", meanings[i]);
            } else {
                println!("{}", meanings[i].green());
            }
            i += 1;
        }
    }
}

fn output_json(output: &Output) {
    let json = serde_json::to_string(output);
    match json {
        Ok(s) => {
            println!("{}", s);
        }
        Err(e) => {
            eprintln!("Json decode error: {}", e);
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Phonetic {
    Na,
    Us,
    Uk,
    Both,
}

#[derive(Debug, Serialize)]
struct Output {
    word: String,
    phonetic_us: Option<String>,
    phonetic_uk: Option<String>,
    audio_us: Option<String>,
    audio_uk: Option<String>,
    pos: Option<Vec<String>>,
    meanings: Option<Vec<String>>,
    desc: Option<String>,
}

impl Output {
    fn new(word: &str) -> Self {
        Self {
            word: word.to_string(),
            phonetic_us: None,
            phonetic_uk: None,
            audio_us: None,
            audio_uk: None,
            pos: None,
            meanings: None,
            desc: None,
        }
    }
}

trait Translation {
    fn translate(&self) -> Result<Output, Box<dyn std::error::Error>>;
}

#[derive(Parser, Debug)]
#[command(
    name = "yi",
    version,
    about = "A fast and simple command-line translation tool."
)]
struct Args {
    word: Option<String>,
    #[arg(long, default_value_t = false, help = "美音朗读")]
    speak_us: bool,
    #[arg(long, default_value_t = false, help = "英音朗读")]
    speak_uk: bool,
    #[arg(long, default_value_t = false, help = "以JSON格式输出")]
    json: bool,
    #[arg(long, default_value_t = false, help = "以无格式纯文本输出")]
    pure: bool,
    #[arg(
        short,
        long,
        default_value = "iciba",
        help = "翻译的后端:\"iciba\" 或者 \"baidu\", 如果是baidu，在环境变量指定:\nexport BAIDU_TRANS_APPID=\"your appid\"\nexport BAIDU_TRANS_KEY=\"your key\""
    )]
    backend: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // parse args
    let args = Args::parse();

    let word = if let Some(w) = args.word {
        w
    } else {
        let mut s = String::new();
        io::stdin().read_to_string(&mut s)?;
        s
    };

    let client = Client::builder().timeout(Duration::from_secs(10)).build()?;

    let appid = env::var("BAIDU_TRANS_APPID").unwrap_or("".to_string());
    let key = env::var("BAIDU_TRANS_KEY").unwrap_or("".to_string());

    let backend: Box<dyn Translation> = match args.backend.as_deref() {
        Some("baidu") => Box::new(Baidu {
            word: &word,
            client: &client,
            appid: &appid,
            key: &key,
        }),
        _ => Box::new(Iciba {
            word: &word,
            client: &client,
        }),
    };

    let output = backend.translate();

    let speak_type = if args.speak_uk && args.speak_us {
        Phonetic::Both
    } else {
        if args.speak_uk {
            Phonetic::Uk
        } else {
            if args.speak_us {
                Phonetic::Us
            } else {
                Phonetic::Na
            }
        }
    };

    match output {
        Ok(output) => {
            if args.json {
                output_json(&output);
            } else if args.pure {
                output_text(&output, true);
            } else {
                output_text(&output, false);
            }
        }
        Err(e) => {
            eprintln!("Translate error {}", e);
            return Err(e);
        }
    }

    let mut speak_rt = Ok(());
    match speak_type {
        Phonetic::Both => {
            let r1 = speak(word.as_str(), Phonetic::Us, &client);
            let r2 = speak(word.as_str(), Phonetic::Uk, &client);
            speak_rt = r1.and(r2)
        }
        Phonetic::Na => {}
        _ => {
            speak_rt = speak(word.as_str(), speak_type, &client);
        }
    }

    match speak_rt {
        Err(e) => {
            eprintln!("speak error {}", e);
        }
        _ => {}
    }

    Ok(())
}

mod tests {
    use super::*;

    #[test]
    fn test_baidu() {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        let appid = env::var("BAIDU_TRANS_APPID").unwrap_or("".to_string());
        let key = env::var("BAIDU_TRANS_KEY").unwrap_or("".to_string());

        let word = "hello";
        let baidu = Baidu {
            word: &word,
            client: &client,
            appid: &appid,
            key: &key,
        };

        let output = baidu.translate().unwrap();
        output_pure(&output);
    }
}
