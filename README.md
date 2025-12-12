# yi [译]

A fast and simple command-line translation tool implemented with Rust.

Currently, only English to Chinese translation is supported.

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
      --speak-us  美音朗读
      --speak-uk  英音朗读
      --json      以JSON格式输出
  -h, --help      Print help
  -V, --version   Print version

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

