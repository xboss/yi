use std::io::{self, Cursor, Read};
use std::time::Duration;

use clap::{Parser, command};
use owo_colors::{OwoColorize, colors::*};
use quick_xml::Reader;
use quick_xml::events::Event;
use reqwest::blocking::Client;
use rodio::{Decoder, OutputStreamBuilder, Sink};
use serde::{Serialize};

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
        // println!("resp: {:#?}", resp);

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
                }
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
            _=>(),
        }
        output.meanings = Some(acceptation_list);
        output.pos = Some(pos_list);
        Ok(output)
    }
}

// app

fn speak(word: &str, phonetic: Phonetic, client: &Client) -> Result<(), Box<dyn std::error::Error>> {

    let ps = if phonetic == Phonetic::Us { println!("美音朗读..."); 2 } else { println!("英音朗读..."); 1 };
    let url = format!("https://dict.youdao.com/dictvoice?audio={}&type={}", word, ps);

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

fn output_text(output: &Output) {
    println!("{}", output.word.fg::<Cyan>());
    if let Some(ps) = output.phonetic_uk.as_ref() {
        println!("英 /{}/ 美 /{}/", ps.green(), output.phonetic_us.as_ref().unwrap().green());
    } else {
        if let Some(ps) = output.phonetic_us.as_ref() {
            println!("/{}/", ps.green());
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
                    println!("{}", meanings[i].green());
                }
                None => {}
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
        Self{
            word: word.to_string(),
            phonetic_us: None,
            phonetic_uk: None,
            audio_us: None,
            audio_uk: None,
            pos: None,
            meanings: None,
            desc: None
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

    let iciba = Iciba {
        word: &word,
        client: &client,
    };

    let output = iciba.translate();

    let speak_type = if args.speak_uk && args.speak_us {Phonetic::Both} else {if args.speak_uk {Phonetic::Uk} else {if args.speak_us {Phonetic::Us} else {Phonetic::Na}}}; 

    match output {
        Ok(output) => {
            if args.json {
                output_json(&output);
            } else {
                output_text(&output);
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
        _ =>{}
    }
    
    // // normal usage
    // println!("{}", "green".green());
    // println!("{}", "yellow".yellow());
    // println!("{}", "blue".blue());
    // println!("{}", "black".black());
    //
    // // generic examples
    // println!("{}", "red".fg::<Red>());
    // println!("{}", "magenta".fg::<Magenta>());
    // println!("{}", "white".fg::<White>());
    // println!("{}", "cyan".fg::<Cyan>());
    //
    // println!("\nBrights\n-------");
    // println!("{}", "green".fg::<BrightGreen>());
    // println!("{}", "yellow".fg::<BrightYellow>());
    // println!("{}", "blue".fg::<BrightBlue>());
    // println!("{}", "black".fg::<BrightBlack>());
    // println!("{}", "red".fg::<BrightRed>());
    // println!("{}", "magenta".fg::<BrightMagenta>());
    // println!("{}", "white".fg::<BrightWhite>());
    // println!("{}", "cyan".fg::<BrightCyan>());

    Ok(())
}
