//! 对实时上传逐块确认顶层 `stream`。状态只与容器嵌套深度有关，不保留属性名或字符串值。

/// 一个容器当前允许的下一个符号；根值也占一格，以便单独识别尾随垃圾。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Want {
    /// 根文档尚未开始。
    Root,
    /// 根文档已经结束，只允许空白。
    End,
    /// 刚进入对象，可以是闭括号。
    FirstKey,
    /// 逗号之后必须有键，不能直接闭合。
    Key,
    /// 键之后必须有冒号。
    Colon,
    /// 冒号之后必须有值。
    Member,
    /// 对象里的值结束，等逗号或闭括号。
    ObjectEnd,
    /// 刚进入数组，可以为空。
    FirstItem,
    /// 数组的逗号之后必须有值。
    Item,
    /// 数组里的值结束，等逗号或闭括号。
    ArrayEnd,
}

/// 数字只需验证拼写，不把任意长的尾数或指数累积成数值。
#[derive(Clone, Copy)]
enum Number {
    /// 负号后至少一位数字。
    Minus,
    /// 整数零不许直接接另一位数字。
    Zero,
    /// 非零整数允许后续数字。
    Int,
    /// 小数点后至少一位数字。
    Dot,
    /// 小数部分允许后续数字或指数。
    Frac,
    /// 指数标记后可以有符号。
    E,
    /// 指数符号后必须有数字。
    Sign,
    /// 指数数字不得再接小数点。
    Exp,
}

/// 跨块的词法状态。字面量引用静态后缀，字符串最多只保留一个尚未完整的字符。
enum Token {
    /// 正在容器语法中，不在原子值内部。
    None,
    /// `true`、`false` 或 `null` 还差的字节。
    Word(&'static [u8]),
    /// 可能还会延伸到下一块的数值。
    Number(Number),
    /// 键或字符串值，用同一个有界字符校验器。
    String(Text),
}

/// 一个 UTF-8 字符或一对 UTF-16 转义；属性名只与固定的 ASCII 字面量增量比较。
struct Text {
    /// 可用于给 serde 加上两端引号的最大 `\uXXXX\uXXXX` 片段。
    part: [u8; 14],
    /// 尚未完成的字符已有多少输入字节。
    used: usize,
    /// 此字符（或 Unicode 代理对）需要的输入长度。
    need: usize,
    /// 匹配到 `stream` 的第几个字节；非目标串不再比较。
    matched: Option<usize>,
}
impl Text {
    /// 只有顶层键才参与匹配，所有字符串都验证语法。
    fn new(compare: bool) -> Self {
        Self {
            part: [b'"'; 14],
            used: 0,
            need: 0,
            matched: compare.then_some(0),
        }
    }
    /// 输入一个字节，返回是否遇到了不在转义内的结束引号。
    fn push(&mut self, b: u8) -> Result<bool, ()> {
        if self.used == 0 {
            self.need = match b {
                b'"' => return Ok(true),
                0..=31 | 0x80..=0xc1 | 0xf5..=0xff => return Err(()),
                b'\\' | 0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => {
                    self.compare(b);
                    return Ok(false);
                }
            };
        }
        self.used += 1;
        self.part[self.used] = b;
        if self.part[1] == b'\\' && self.used == 2 && b == b'u' {
            self.need = 6;
        }
        if self.used != self.need {
            return Ok(false);
        }
        if self.need == 6
            && self.part[3].eq_ignore_ascii_case(&b'd')
            && matches!(self.part[4], b'8' | b'9' | b'a' | b'b' | b'A' | b'B')
        {
            self.need = 12;
            return Ok(false);
        }
        self.part[self.used + 1] = b'"';
        let c: char = serde_json::from_slice(&self.part[..self.used + 2]).map_err(|_| ())?;
        self.compare(if c.is_ascii() { c as u8 } else { 0 });
        self.used = 0;
        Ok(false)
    }
    /// 一旦失配就不增长任何计数；即使一个键有几 GiB 也只占常数空间。
    fn compare(&mut self, b: u8) {
        self.matched = self
            .matched
            .and_then(|i| (b"stream".get(i) == Some(&b)).then_some(i + 1));
    }
}

/// 未完成的实时 JSON 探针。超过深度预算时保留未知，坏 JSON 与普通非流请求同样判为 false。
pub(super) struct LiveFlag {
    /// 每层一个语法状态；仅对实际出现的嵌套分配，最多 128 层。
    stack: Vec<Want>,
    /// 跨块的原子值或键。
    token: Token,
    /// 下一个值是否属于根对象的 stream 键。
    selected: bool,
    /// 最后一次匹配的布尔值；仍需等合法文档 EOF 才能发布。
    flag: bool,
    /// 达到嵌套预算时不能把未知当作 false 发布。
    unknown: bool,
}
impl LiveFlag {
    /// 初始时未看到根值，也没有任何投票。
    pub(super) fn new() -> Self {
        Self {
            stack: vec![Want::Root],
            token: Token::None,
            selected: false,
            flag: false,
            unknown: false,
        }
    }
    /// 只处理现在已经收到的字节；慢客户端停顿时没有线程或异步任务等待下一块。
    pub(super) fn feed(&mut self, bytes: &[u8]) {
        if self.stack.is_empty() {
            return;
        }
        for &b in bytes {
            if self.byte(b).is_err() {
                self.stack.clear();
                break;
            }
        }
    }
    /// 值开始时即设置父容器的下一状态；子容器闭合时只需弹栈，尾随逗号不会被当作空容器。
    fn byte(&mut self, b: u8) -> Result<(), ()> {
        use Number::*;
        use Want::*;
        match &mut self.token {
            Token::Word(word) => {
                if word.first() != Some(&b) {
                    return Err(());
                }
                *word = &word[1..];
                if word.is_empty() {
                    self.token = Token::None;
                }
                return Ok(());
            }
            Token::String(text) => {
                if text.push(b)? {
                    if self.stack.last() == Some(&Colon) {
                        self.selected = text.matched == Some(6);
                    }
                    self.token = Token::None;
                }
                return Ok(());
            }
            Token::Number(n) => match (*n, b) {
                (Minus, b'0') => {
                    *n = Zero;
                    return Ok(());
                }
                (Minus, b'1'..=b'9') | (Int, b'0'..=b'9') => {
                    *n = Int;
                    return Ok(());
                }
                (Zero | Int, b'.') => {
                    *n = Dot;
                    return Ok(());
                }
                (Dot | Frac, b'0'..=b'9') => {
                    *n = Frac;
                    return Ok(());
                }
                (Zero | Int | Frac, b'e' | b'E') => {
                    *n = E;
                    return Ok(());
                }
                (E, b'+' | b'-') => {
                    *n = Sign;
                    return Ok(());
                }
                (E | Sign | Exp, b'0'..=b'9') => {
                    *n = Exp;
                    return Ok(());
                }
                (Zero | Int | Frac | Exp, _) => self.token = Token::None,
                _ => return Err(()),
            },
            Token::None => (),
        }
        if matches!(b, b' ' | b'\t' | b'\r' | b'\n') {
            return Ok(());
        }
        let depth = self.stack.len();
        let want = self.stack.last_mut().ok_or(())?;
        match (*want, b) {
            (FirstKey | ObjectEnd, b'}') | (FirstItem | ArrayEnd, b']') => {
                self.stack.pop();
            }
            (FirstKey | Key, b'"') => {
                *want = Colon;
                self.token = Token::String(Text::new(depth == 2));
            }
            (Colon, b':') => *want = Member,
            (ObjectEnd, b',') => *want = Key,
            (ArrayEnd, b',') => *want = Item,
            (Root | Member | FirstItem | Item, _) => {
                if std::mem::take(&mut self.selected) {
                    self.flag = b == b't';
                }
                *want = match *want {
                    Root => End,
                    Member => ObjectEnd,
                    _ => ArrayEnd,
                };
                self.token = match b {
                    b'{' | b'[' => {
                        if depth > 128 {
                            self.unknown = true;
                            return Err(());
                        }
                        self.stack
                            .push(if b == b'{' { FirstKey } else { FirstItem });
                        Token::None
                    }
                    b'"' => Token::String(Text::new(false)),
                    b't' => Token::Word(b"rue"),
                    b'f' => Token::Word(b"alse"),
                    b'n' => Token::Word(b"ull"),
                    b'-' => Token::Number(Minus),
                    b'0' => Token::Number(Zero),
                    b'1'..=b'9' => Token::Number(Int),
                    _ => return Err(()),
                };
            }
            _ => return Err(()),
        }
        Ok(())
    }
    /// 只有内容完整的根对象能给出 true；取消上传的调用方根本不结算。
    pub(super) fn finish(self) -> Option<bool> {
        (!self.unknown)
            .then_some(self.flag && self.stack == [Want::End] && matches!(self.token, Token::None))
    }
}

#[cfg(test)]
mod tests {
    use super::LiveFlag;

    /// 分块点可以落在键、数值、转义代理对或 UTF-8 字符内部；整份文档均合法才接受最后的 stream。
    #[test]
    fn live_flag_matches_complete_json_across_byte_boundaries() {
        let cases: &[&[u8]] = &[
            br#"{"stream":false,"\u0073tr\u0065am":true}"#,
            "{\"stream\":true,\"值\":[-0.4e-17,0,12345,\"🦀\"]}".as_bytes(),
            br#"{"stream":true,"s":"\uD83E\uDD80\r\n\t\/\\\""}"#,
            br#"{"stream":true,"x":{"stream":false},"v":[{},[],true,false,null]}"#,
            br#"{"stream":true,"stream":{"stream":true}}"#,
            br#"[{"stream":true}]"#,
            br#"{"stream":true,"stream":"true"}"#,
            br#"{"stream":true} bad"#,
            br#"{"stream":true,}"#,
            br#"{"stream":true,"x":[0,]}"#,
            br#"{"stream":true,"x":01}"#,
            br#"{"stream":true,"x":1e-}"#,
            br#"{"stream":true,"x":"\uD800"}"#,
            br#"{"stream":true,"x":"\uDC00"}"#,
            b"{\"stream\":true,\"x\":\"\xC0\xBD\"}",
            br#"{"stream":true"#,
        ];
        for &bytes in cases {
            let expected = serde_json::from_slice::<serde_json::Value>(bytes)
                .ok()
                .and_then(|v| v.get("stream").and_then(|x| x.as_bool()))
                .unwrap_or(false);
            for step in [1, 2, 7, 256] {
                let mut probe = LiveFlag::new();
                for part in bytes.chunks(step) {
                    probe.feed(part);
                }
                assert_eq!(probe.finish(), Some(expected), "{bytes:?}, split {step}");
            }
        }
    }
    /// 即使是合法但极深的输入也不增长栈；未知不同于已经确认不是流。
    #[test]
    fn live_flag_bounds_nested_containers() {
        let mut probe = LiveFlag::new();
        probe.feed(br#"{"stream":true,"x":"#);
        probe.feed(&[b'['; 1024]);
        probe.feed(&[b']'; 1024]);
        probe.feed(b"}");
        assert_eq!(probe.finish(), None);
    }
}
