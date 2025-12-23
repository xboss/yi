# yi [译]

A fast and simple command-line translation tool implemented with Rust.

Except for using Baidu backend, only English to Chinese translation is supported.

## Build

```
cargo build --release
```

## Usage

```
cd target/release/

./yi --help
A fast and simple command-line translation tool.

Usage: yi [OPTIONS] [WORD]

Arguments:
  [WORD]

Options:
      --speak-us           美音朗读
      --speak-uk           英音朗读
      --json               以JSON格式输出
      --pure               以无格式纯文本输出
  -b, --backend <BACKEND>  翻译的后端:"iciba" 或者 "baidu", 如果是baidu，在环境变量指定:
                           export BAIDU_TRANS_APPID="your appid"
                           export BAIDU_TRANS_KEY="your key" [default: iciba]
  -h, --help               Print help
  -V, --version            Print version

```

```
./yi  nice
nice
英 /naɪs/ 美 /naɪs/
adj. 美好的，愉快的；正派的；友好的，亲切的；细致的；
```

```
./yi --speak-us --speak-uk nice
nice
英 /naɪs/ 美 /naɪs/
adj. 美好的，愉快的；正派的；友好的，亲切的；细致的；
美音朗读...
英音朗读...
```

```
./yi --json nice
{"word":"nice","phonetic_us":"naɪs","phonetic_uk":"naɪs","audio_us":null,"audio_uk":null,"pos":["adj."],"meanings":["美好的，愉快的；正派的；友好的，亲切的；细致的；"],"desc":null}
```
or
```
echo "nice" | ./yi 
nice
英 /naɪs/ 美 /naɪs/
adj. 美好的，愉快的；正派的；友好的，亲切的；细致的；
```

