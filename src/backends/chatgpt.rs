use crate::translation::{Translation, Output};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use anyhow::{Result, bail};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};

const URL_CHATGPT: &str = "https://api.openai.com/v1/responses";
const DEF_MODEL: &str  = "gpt-4o-mini";
const DEF_CONTENT: &str = r"你是一本专业的中英文双语词典。请按照以下要求提供翻译和解释：

1. 格式要求：
   [原词] [音标] ~ [翻译] [拼音]

   - [词性] [释义1]
   - [词性] [释义2]
   ...

   例句：
   1. [原文例句]
      [翻译]
   2. [原文例句]
      [翻译]
   ...

   -----
2. 翻译规则：
   - 英文输入翻译为中文，中文输入翻译为英文
   - 提供准确的音标（英文）或拼音（中文）
   - 列出所有常见词性及其对应的释义
   - 释义应简洁明了，涵盖词语的主要含义，使用中文
   - 提供2-3个地道的例句，体现词语的不同用法和语境
3. 内容质量：
   - 确保翻译和释义的准确性和权威性
   - 例句应当实用、常见，并能体现词语的典型用法
   - 注意词语的语体色彩，如正式、口语、书面语等
   - 对于多义词，按照使用频率由高到低排列释义
4. 特殊情况：
   - 对于习语、谚语或特殊表达，提供对应的解释和等效表达
   - 注明词语的使用范围，如地域、行业特定用语等
   - 对于缩写词，提供完整形式和解释
请基于以上要求，为用户提供简洁、专业、全面且易于理解的词语翻译和解释。

要翻译的单词是: ";

#[derive(Debug)]
pub struct Chatgpt<'a> {
    pub word: &'a str,
    pub client: &'a Client,
    pub key: &'a str,
}

#[derive(Debug, Serialize)]
struct ChatgptRequest<'a> {
    model: &'a str,
    input: &'a str,
}

#[derive(Deserialize, Debug)]
struct ChatgptResponse {
    status: Option<String>,
    output: Option<Vec<ChatgptTextOutput>>,
}

#[derive(Deserialize, Debug)]
struct ChatgptTextOutput {
    id: Option<String>,
    #[serde(rename = "type")]
    text_type: Option<String>,
    role: Option<String>,
    content: Option<Vec<ChatgptTextContent>>,
}

#[derive(Deserialize, Debug)]
struct ChatgptTextContent {
    #[serde(rename = "type")]
    content_type: Option<String>,
    text: Option<String>,
}

impl<'a> Translation for Chatgpt<'a> {
    fn translate(&self) -> Result<Output> {

        let input = format!("{} {}", DEF_CONTENT, self.word);
        
        let request = ChatgptRequest {
            model: DEF_MODEL,
            input: input.as_str(),
        };

        let resp = self
            .client
            .post(URL_CHATGPT)
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, format!("Bearer {}", self.key))
            .json(&request)
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

        let resp = serde_json::from_str::<ChatgptResponse>(&resp)?;

        if resp.status.as_deref() != Some("completed") {
            bail!("chat gpt status error.");
        }

        if let Some(resp_outputs) = resp.output{
            for resp_output in resp_outputs {
                if resp_output.role.as_deref() == Some("assistant") {
                    if let Some(contents) = resp_output.content {
                        output.meanings = Some(Vec::new());
                        for c in contents {
                            if c.content_type.as_deref() == Some("output_text") {
                                if let Some(text) = c.text {
                                    if let Some(m) = &mut output.meanings {
                                        m.push(text);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        
        Ok(output)
    }
}
