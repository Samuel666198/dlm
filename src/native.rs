//! Chrome/Edge Native Messaging 的 stdio 帧（B1）。
//!
//! 浏览器契约：每帧 = 4 字节**本机字节序**长度 + UTF-8 JSON。宿主只能往 stdout 写帧，
//! 诊断走 stderr。这里只做最小 ping/pong；协议自定、可读 JSON，不复刻 IDM 的数字操作码。

use std::io::{self, Read, Write};

use serde_json::{Value, json};

/// 与扩展/宿主 manifest 里的 `name` 同源。
pub const HOST_NAME: &str = "com.dlm.host";
/// 本宿主接受的单帧上限。Chrome 对宿主→浏览器是 1 MB；浏览器→宿主更宽，但 B1 不需要。
pub const MAX_MESSAGE_BYTES: u32 = 1024 * 1024;

/// 读一帧；干净 EOF 交 `None`，半截长度/半截体交 `UnexpectedEof`。
pub fn read_frame<R: Read>(r: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut len_bytes = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match r.read(&mut len_bytes[got..]) {
            Ok(0) => {
                if got == 0 {
                    return Ok(None);
                }
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "native message length truncated",
                ));
            }
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    let len = u32::from_ne_bytes(len_bytes);
    if len > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("native message too large: {len} > {MAX_MESSAGE_BYTES}"),
        ));
    }
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload)?;
    Ok(Some(payload))
}

/// 写一帧并 flush；stdout 上不许出现帧以外的东西。
pub fn write_frame<W: Write>(w: &mut W, payload: &[u8]) -> io::Result<()> {
    let len = u32::try_from(payload.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "native message length overflows u32",
        )
    })?;
    if len > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("native message too large: {len} > {MAX_MESSAGE_BYTES}"),
        ));
    }
    w.write_all(&len.to_ne_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

/// 一条请求帧 → 一条响应帧。未知 op 与坏 JSON 都回可读错误，不回显原始串。
pub fn respond(payload: &[u8]) -> Vec<u8> {
    let value: Value = match serde_json::from_slice(payload) {
        Ok(v) => v,
        Err(_) => {
            return serde_json::to_vec(&json!({"op": "error", "message": "invalid json"}))
                .expect("json! serializable");
        }
    };
    let op = value.get("op").and_then(Value::as_str).unwrap_or("");
    let reply = match op {
        "ping" => versioned("pong", &value),
        "hello" => versioned("hello", &value),
        _ => json!({"op": "error", "message": "unknown op"}),
    };
    serde_json::to_vec(&reply).expect("json! serializable")
}

/// 带双向版本号的回执。
///
/// ## 为什么回执里有两个内核版本字段
///
/// `version` 是 C3 时期就有的名字，已经在扩展里读着（`popup.js` 显示「宿主 0.1.0」）。
/// `core_version` 是同一件东西的**明确**名字：清单里 `core` 指的就是它。
/// 两个都发，老扩展不会因为改名而失去显示，新代码读语义清楚的那个。
///
/// ## `ext_version` 是回显
///
/// 扩展报上来的 `manifest.version` 原样带回：一来确认收到，二来扩展只读一条回执
/// 就能同时显示「宿主 X · 扩展 Y」，不用自己拼。
fn versioned(op: &str, request: &Value) -> Value {
    let mut reply = json!({
        "op": op,
        "host": HOST_NAME,
        "version": env!("CARGO_PKG_VERSION"),
        "core_version": env!("CARGO_PKG_VERSION"),
    });
    if let Some(ext) = request
        .get("ext_version")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && let Some(obj) = reply.as_object_mut()
    {
        obj.insert("ext_version".into(), Value::String(ext.to_string()));
    }
    reply
}

/// 一直服务到 stdin EOF；每一帧都回一帧。
pub fn serve<R: Read, W: Write>(mut input: R, mut output: W) -> io::Result<()> {
    while let Some(frame) = read_frame(&mut input)? {
        let reply = respond(&frame);
        write_frame(&mut output, &reply)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn framed(payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        write_frame(&mut out, payload).unwrap();
        out
    }

    #[test]
    fn frame_roundtrips_through_native_endian_length() {
        let payload = br#"{"op":"ping"}"#;
        let bytes = framed(payload);
        assert_eq!(&bytes[..4], &(payload.len() as u32).to_ne_bytes());
        let mut cur = Cursor::new(bytes);
        assert_eq!(read_frame(&mut cur).unwrap().unwrap(), payload);
        assert_eq!(read_frame(&mut cur).unwrap(), None);
    }

    #[test]
    fn empty_input_is_clean_eof_and_partial_length_is_an_error() {
        let mut empty = Cursor::new(Vec::<u8>::new());
        assert!(read_frame(&mut empty).unwrap().is_none());

        let mut half = Cursor::new(vec![1u8, 0u8]);
        assert_eq!(
            read_frame(&mut half).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn oversized_length_is_refused_before_allocating_the_body() {
        let mut cur = Cursor::new((MAX_MESSAGE_BYTES + 1).to_ne_bytes().to_vec());
        assert_eq!(
            read_frame(&mut cur).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn oversized_write_is_refused() {
        let payload = vec![0u8; MAX_MESSAGE_BYTES as usize + 1];
        let mut out = Vec::new();
        assert_eq!(
            write_frame(&mut out, &payload).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn ping_gets_pong_with_host_and_version() {
        let reply: Value = serde_json::from_slice(&respond(br#"{"op":"ping"}"#)).unwrap();
        assert_eq!(reply["op"], "pong");
        assert_eq!(reply["host"], HOST_NAME);
        assert_eq!(reply["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(
            reply["core_version"], reply["version"],
            "两个名字指的是同一件东西：清单里的 core"
        );
        assert!(
            reply.get("ext_version").is_none(),
            "扩展没报版本时不该凭空造一个字段：{reply}"
        );
    }

    /// 双向版本号：扩展报上来的 `ext_version` 原样带回，扩展只读一条回执就能显示两边版本。
    #[test]
    fn ping_echoes_the_extension_version_it_was_given() {
        let reply: Value =
            serde_json::from_slice(&respond(br#"{"op":"ping","ext_version":"0.2.12"}"#)).unwrap();
        assert_eq!(reply["ext_version"], "0.2.12");
        assert_eq!(reply["core_version"], env!("CARGO_PKG_VERSION"));

        let hello: Value =
            serde_json::from_slice(&respond(br#"{"op":"hello","ext_version":" "}"#)).unwrap();
        assert!(
            hello.get("ext_version").is_none(),
            "空白版本号当作没报：{hello}"
        );
    }

    #[test]
    fn hello_unknown_and_invalid_json_are_readable() {
        let hello: Value = serde_json::from_slice(&respond(br#"{"op":"hello"}"#)).unwrap();
        assert_eq!(hello["op"], "hello");
        assert_eq!(hello["host"], HOST_NAME);

        let unknown: Value = serde_json::from_slice(&respond(br#"{"op":"wat"}"#)).unwrap();
        assert_eq!(unknown["op"], "error");
        assert_eq!(unknown["message"], "unknown op");

        let bad: Value = serde_json::from_slice(&respond(b"not json")).unwrap();
        assert_eq!(bad["op"], "error");
        assert_eq!(bad["message"], "invalid json");
    }

    #[test]
    fn serve_replies_to_each_frame_until_eof() {
        let mut input = Vec::new();
        input.extend(framed(br#"{"op":"ping"}"#));
        input.extend(framed(br#"{"op":"wat"}"#));
        let mut output = Vec::new();
        serve(Cursor::new(input), &mut output).unwrap();
        let mut cur = Cursor::new(output);
        let first: Value = serde_json::from_slice(&read_frame(&mut cur).unwrap().unwrap()).unwrap();
        let second: Value =
            serde_json::from_slice(&read_frame(&mut cur).unwrap().unwrap()).unwrap();
        assert_eq!(first["op"], "pong");
        assert_eq!(second["op"], "error");
        assert!(read_frame(&mut cur).unwrap().is_none());
    }
}
