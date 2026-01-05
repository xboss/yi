use std::io::{self, Cursor, Read};
use std::time::Duration;
use clap::{Parser, command};
use owo_colors::{OwoColorize, colors::*};
use reqwest::blocking::Client;
use rodio::{Decoder, OutputStreamBuilder, Sink};
// use serde::{Deserialize, Serialize};
use anyhow::{Result, bail};
use std::env;

mod translation;
use crate::translation::{Translation, Output};

mod backends;
use backends::iciba::Iciba;
use backends::baidu::Baidu;
use backends::chatgpt::Chatgpt;

fn speak(word: &str, phonetic: Phonetic, client: &Client) -> Result<()> {
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
            bail!("Error: audio request timed out.");
        }
        Err(e) => {
            bail!("Network error: {:?}", e);
        }
    };

    // check status code
    if !resp.status().is_success() {
        bail!("HTTP error: {}", resp.status());
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
    if is_pure {
        println!("{}", output.word);
    } else {
        println!("{}", output.word.fg::<Cyan>());
    }
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
        help = "翻译的后端:\"iciba\" 、 \"baidu\" 、 \"chatgpt\" \n如果是baidu，在环境变量指定:\n\texport BAIDU_TRANS_APPID=\"your appid\"\n\texport BAIDU_TRANS_KEY=\"your key\" \n如果是\"chagpt\"，在环境变量指定:\n\texport OPENAI_API_KEY=\"your key\"\n"
    )]
    backend: Option<String>,
    #[arg(
        short,
        long,
        help = "支持socks5代理：sock5h://127.0.0.1:1080"
    )]
    proxy: Option<String>,
}

fn main() -> Result<()> {
    // parse args
    let args = Args::parse();

    let word = if let Some(w) = args.word {
        w
    } else {
        let mut s = String::new();
        io::stdin().read_to_string(&mut s)?;
        s
    };

    let mut client_builder = Client::builder();
    if let Some(proxy_str) = args.proxy {
        let proxy = reqwest::Proxy::all(proxy_str)?;
        client_builder = client_builder.proxy(proxy);
    }
    
    let client = client_builder.timeout(Duration::from_secs(100)).build()?;

    let appid = env::var("BAIDU_TRANS_APPID").unwrap_or("".to_string());
    let baidu_key = env::var("BAIDU_TRANS_KEY").unwrap_or("".to_string());
    let chatgpt_key = env::var("OPENAI_API_KEY").unwrap_or("".to_string());

    let backend: Box<dyn Translation> = match args.backend.as_deref() {
        Some("baidu") => Box::new(Baidu {
            word: &word,
            client: &client,
            appid: &appid,
            key: &baidu_key,
        }),
        Some("chatgpt") => Box::new(Chatgpt {
            word: &word,
            client: &client,
            key: &chatgpt_key,
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

// mod tests {
//     use super::*;

//     #[test]
//     fn test_baidu() {
//         let client = Client::builder()
//             .timeout(Duration::from_secs(10))
//             .build()
//             .unwrap();
//         let appid = env::var("BAIDU_TRANS_APPID").unwrap_or("".to_string());
//         let key = env::var("BAIDU_TRANS_KEY").unwrap_or("".to_string());

//         let word = "hello";
//         let baidu = Baidu {
//             word: &word,
//             client: &client,
//             appid: &appid,
//             key: &key,
//         };

//         let output = baidu.translate().unwrap();
//         output_text(&output, true);
//     }
// }
