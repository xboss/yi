use anyhow::{Result, bail};
use crate::translation::{Translation, Output};
use reqwest::blocking::Client;
use quick_xml::Reader;
use quick_xml::events::Event;

#[derive(Debug)]
pub struct Iciba<'a> {
    pub word: &'a str,
    pub client: &'a Client,
}

impl<'a> Translation for Iciba<'a> {
    fn translate(&self) -> Result<Output> {
        const URL_ICIBA: &str = "https://dict-co.iciba.com/api/dictionary.php";
        let params = [
            ("key", "D191EBD014295E913574E1EAF8E06666"),
            ("w", &self.word),
        ];

        let resp = self.client.get(URL_ICIBA).query(&params).send()?;

        // check status code
        if !resp.status().is_success() {
            bail!("HTTP error: {}", resp.status())
        }

        let resp = match resp.text() {
            Ok(t) => t,
            Err(e) => {
                bail!("HTTP response error: {:?}", e);
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
                Err(e) => bail!("Error at postion {}: {:?}", reader.error_position(), e),
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
