use super::*;
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Status,
    Models,
    Generate(GenerationRequest),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub command: Command,
}
pub fn decode_hex(value: &str) -> Result<String, ForgeError> {
    if value.len() % 2 != 0 {
        return Err(ForgeError::InvalidRequest);
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
        fn digit(b: u8) -> Result<u8, ForgeError> {
            match b {
                b'0'..=b'9' => Ok(b - b'0'),
                b'a'..=b'f' => Ok(b - b'a' + 10),
                b'A'..=b'F' => Ok(b - b'A' + 10),
                _ => Err(ForgeError::InvalidRequest),
            }
        }
        bytes.push(digit(pair[0])? * 16 + digit(pair[1])?);
    }
    String::from_utf8(bytes).map_err(|_| ForgeError::InvalidRequest)
}
pub fn encode_hex(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 2);
    for b in value.bytes() {
        out.push(char::from(b"0123456789abcdef"[(b >> 4) as usize]));
        out.push(char::from(b"0123456789abcdef"[(b & 15) as usize]));
    }
    out
}
impl Request {
    pub fn parse(line: &str) -> Result<Self, ForgeError> {
        if line.len() > MAX_FRAME_BYTES {
            return Err(ForgeError::InvalidRequest);
        }
        let fields: Vec<&str> = line.trim_end_matches(['\r', '\n']).split('\t').collect();
        if fields.len() < 3 || fields[0] != "FORGE1" {
            return Err(ForgeError::InvalidRequest);
        }
        let id = fields[1].parse().map_err(|_| ForgeError::InvalidRequest)?;
        let command = match (fields[2], fields.len()) {
            ("status", 3) => Command::Status,
            ("models", 3) => Command::Models,
            ("generate", 6) => {
                let req = GenerationRequest {
                    model: if fields[3] == "-" {
                        None
                    } else {
                        Some(fields[3].to_string())
                    },
                    max_tokens: fields[4].parse().map_err(|_| ForgeError::InvalidRequest)?,
                    prompt: decode_hex(fields[5])?,
                };
                req.validate()?;
                Command::Generate(req)
            }
            _ => return Err(ForgeError::InvalidRequest),
        };
        Ok(Self { id, command })
    }
}
pub fn response(id: u64, result: Result<String, ForgeError>) -> String {
    match result {
        Ok(text) => format!("FORGE1\t{id}\tok\t{}\n", encode_hex(&text)),
        Err(err) => format!(
            "FORGE1\t{id}\terror\t{}\t{}\n",
            err.code(),
            encode_hex(&err.to_string())
        ),
    }
}
impl Forge {
    pub fn dispatch(&mut self, request: Request) -> String {
        let result = match request.command {
            Command::Generate(r) => self.generate(&r).map(|g| g.text),
            Command::Status | Command::Models => {
                let models = self
                    .models()
                    .iter()
                    .map(|m| {
                        format!(
                            "{{\"id\":\"{}\",\"state\":\"{}\",\"weight_bytes\":{},\"error\":{}}}",
                            m.id,
                            m.state,
                            m.weight_bytes,
                            m.error
                                .map(|e| format!("\"{}\"", e))
                                .unwrap_or_else(|| "null".to_string())
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                Ok(format!(
                    "{{\"default_model\":\"{}\",\"models\":[{}]}}",
                    self.default_model(),
                    models
                ))
            }
        };
        response(request.id, result)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundaries_and_unicode() {
        let text = "こんにちは\n\t";
        assert_eq!(decode_hex(&encode_hex(text)).unwrap(), text);
        for input in [
            "FORGE1\t1\tstatus\textra",
            "FORGE2\t1\tstatus",
            "FORGE1\t1\tgenerate\t-\t257\t61",
            "FORGE1\t1\tgenerate\t-\t1\tff",
        ] {
            assert!(Request::parse(input).is_err());
        }
    }
    #[test]
    fn unavailable_is_not_generated_text() {
        let mut f = Forge::new(0);
        let result = f.dispatch(Request::parse("FORGE1\t9\tgenerate\t-\t8\t6869").unwrap());
        assert!(result.contains("\terror\tweights_missing\t"));
        f.mark_backend_unavailable(DEFAULT_MODEL).unwrap();
        assert!(f
            .dispatch(Request::parse("FORGE1\t9\tgenerate\t-\t8\t6869").unwrap())
            .contains("backend_unavailable"));
    }
}
