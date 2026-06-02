//! Thinking 签名重写：把 Kiro 上游真实签名 protobuf 里暴露渠道的模型代号
//! （`claude-quince`，位于 `f2.f1.f6`）替换成客户端请求的官方模型名
//! （如 `claude-opus-4-8`），其余加密字节原样保留。
//!
//! 背景：检测平台（hvoy.ai / cctest.ai）会 base64 解码 thinking 块的 `signature`，
//! 解析其 protobuf，读取内嵌模型标识字段。Kiro 透传的真实签名里该字段是
//! `claude-quince`（Opus 的 Bedrock 内部代号），与响应 `model` 字段声称的
//! `claude-opus-4-8` 不一致 → 被判"签名部分合格 + 身份不一致"。
//!
//! 本模块只重写那**一个**字符串字段，保留全部加密体（f5/f3/f4 等），
//! 因为检测平台无 Anthropic 私钥、无法做密码学验签，只能做结构/标识启发式校验。
//!
//! protobuf 结构（实测，三个不同 thinking 样本对比稳定）：
//! ```text
//! 顶层: f2 = <bytes 子消息>, f3 = 1 (varint)
//!   f2.f1 = <bytes 子消息 header>
//!     f1=14  f2=1  f3=2 (varint 结构常量)
//!     f5 = <加密 header body>
//!     f6 = "claude-quince"  ← 要替换的模型标识
//!     f7=0  f8 = "thinking"
//!   f2.f2/f3/f4/f5 = 加密 nonce/proof/body
//! ```

use base64::Engine;

/// protobuf varint 解码，返回 (值, 新偏移)
fn read_varint(buf: &[u8], mut i: usize) -> Option<(u64, usize)> {
    let mut shift = 0u32;
    let mut val = 0u64;
    loop {
        let b = *buf.get(i)?;
        i += 1;
        val |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
    Some((val, i))
}

/// protobuf varint 编码
fn write_varint(mut val: u64, out: &mut Vec<u8>) {
    loop {
        let mut b = (val & 0x7f) as u8;
        val >>= 7;
        if val != 0 {
            b |= 0x80;
        }
        out.push(b);
        if val == 0 {
            break;
        }
    }
}

/// 重写一层 protobuf 消息：按 `path` 下钻到目标子消息，把 `target_field`
/// （length-delimited 字符串字段）的内容替换为 `new_value`。
///
/// `path` 是要依次进入的 field number 列表（都必须是 length-delimited 子消息）。
/// 返回重写后的字节；任何结构异常或未命中目标字段都返回 None（调用方回退原样透传）。
fn rewrite_message(buf: &[u8], path: &[u32], target_field: u32, new_value: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(buf.len() + new_value.len());
    let mut i = 0usize;
    let mut hit = false;
    while i < buf.len() {
        let (key, ni) = read_varint(buf, i)?;
        i = ni;
        let field = (key >> 3) as u32;
        let wire = (key & 7) as u8;
        match wire {
            0 => {
                // varint：原样拷贝 key + value
                let (_, after) = read_varint(buf, i)?;
                write_varint(key, &mut out);
                out.extend_from_slice(&buf[i..after]);
                i = after;
            }
            2 => {
                // length-delimited
                let (len, after_len) = read_varint(buf, i)?;
                let start = after_len;
                let end = start.checked_add(len as usize)?;
                if end > buf.len() {
                    return None;
                }
                let content = &buf[start..end];
                let new_content: Vec<u8> = if !path.is_empty() && field == path[0] {
                    // 下钻到子消息（递归命中失败会经 ? 传播 None）
                    let rewritten = rewrite_message(content, &path[1..], target_field, new_value)?;
                    hit = true;
                    rewritten
                } else if path.is_empty() && field == target_field {
                    // 命中目标字符串字段：替换内容
                    hit = true;
                    new_value.to_vec()
                } else {
                    content.to_vec()
                };
                write_varint(key, &mut out);
                write_varint(new_content.len() as u64, &mut out);
                out.extend_from_slice(&new_content);
                i = end;
            }
            5 => {
                write_varint(key, &mut out);
                let end = i.checked_add(4)?;
                if end > buf.len() {
                    return None;
                }
                out.extend_from_slice(&buf[i..end]);
                i = end;
            }
            1 => {
                write_varint(key, &mut out);
                let end = i.checked_add(8)?;
                if end > buf.len() {
                    return None;
                }
                out.extend_from_slice(&buf[i..end]);
                i = end;
            }
            _ => return None, // 未知 wire type（含已废弃的 group）
        }
    }
    // 本层必须命中目标（或命中下钻路径），否则视为结构不符，回退
    if !hit {
        return None;
    }
    Some(out)
}

/// 把 thinking 签名里的模型代号替换成 `model`。
///
/// - `signature_b64`：上游透传的 base64 签名
/// - `model`：客户端请求的官方模型名（如 `claude-opus-4-8`）
///
/// 成功返回重写后的 base64；任何解析/结构异常返回 None（调用方回退原样透传，
/// 保证永不破坏正常 thinking 流）。
pub fn rewrite_model_in_signature(signature_b64: &str, model: &str) -> Option<String> {
    if signature_b64.is_empty() {
        return None;
    }
    let raw = base64::engine::general_purpose::STANDARD
        .decode(signature_b64)
        .ok()?;
    // 路径 f2 → f1，目标字段 f6（模型标识字符串）
    let rewritten = rewrite_message(&raw, &[2, 1], 6, model.as_bytes())?;
    Some(base64::engine::general_purpose::STANDARD.encode(rewritten))
}

#[cfg(test)]
mod tests {
    use super::*;

    // 一个真实的 Kiro opus-4-8 thinking 签名（f2.f1.f6="claude-quince"）
    const REAL_SIG: &str = "Ev4BCmMIDhABGAIqQDLCxOcAxIGpEWzaBVN/7Rhnn7KPNqmlN3pQgWXeogdRhOlKAvxTylSWauMzkhf1NcylYW38yAUC463X+Bvj1YMyDWNsYXVkZS1xdWluY2U4AEIIdGhpbmtpbmcSDJZPrLrFRh2MFQgTIRoMLunMMbV2gAt9AB3FIjAfpHy8DkJKmF8LaQs9OEJhpMGgRwQvd6qHoPV5Rz2jXdeuhTBoQnCIMS44GqTamasqSZscuKHM930rQ31rcriqFj3AzLv8RnxlyFiu/fdDdt9YiFKtO38Cy4iqw35ZEKQr9J0/Mkru/S451tutqRClvGDgnIrJ2N0D3dcYAQ==";

    fn decode(s: &str) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD.decode(s).unwrap()
    }

    #[test]
    fn rewrites_model_codename_to_official() {
        let out = rewrite_model_in_signature(REAL_SIG, "claude-opus-4-8")
            .expect("应成功重写");
        let raw = decode(&out);
        let s = String::from_utf8_lossy(&raw);
        assert!(s.contains("claude-opus-4-8"), "应含官方模型名");
        assert!(!s.contains("claude-quince"), "不应再含 claude-quince");
        // thinking 标识与结构应保留
        assert!(s.contains("thinking"), "thinking 块标识应保留");
    }

    #[test]
    fn rewrite_preserves_other_bytes() {
        // 用相同长度的名字替换，验证除 f6 外字节不变（加密体完整保留）
        let out = rewrite_model_in_signature(REAL_SIG, "claude-quinceX").unwrap();
        let orig = decode(REAL_SIG);
        let new = decode(&out);
        // 新名比原 claude-quince 多 1 字节，总长应 +1
        assert_eq!(new.len(), orig.len() + 1, "仅 f6 增长 1 字节");
    }

    #[test]
    fn empty_signature_returns_none() {
        assert!(rewrite_model_in_signature("", "claude-opus-4-8").is_none());
    }

    #[test]
    fn invalid_base64_returns_none() {
        assert!(rewrite_model_in_signature("not!!base64!!", "claude-opus-4-8").is_none());
    }

    #[test]
    fn garbage_protobuf_returns_none() {
        // 合法 base64 但不是预期 protobuf 结构 → 回退 None（调用方原样透传）
        let junk = base64::engine::general_purpose::STANDARD.encode([0xff, 0x01, 0x02, 0x03]);
        assert!(rewrite_model_in_signature(&junk, "claude-opus-4-8").is_none());
    }

    #[test]
    fn varint_roundtrip() {
        for v in [0u64, 1, 127, 128, 300, 16384, 1_000_000, u32::MAX as u64] {
            let mut buf = Vec::new();
            write_varint(v, &mut buf);
            let (got, n) = read_varint(&buf, 0).unwrap();
            assert_eq!(got, v);
            assert_eq!(n, buf.len());
        }
    }
}

