//! A chat server's cases: prompt enhance's asks (OpenAI-compatible chat
//! completions), as the app sends them.

use std::time::Instant;

use increart_core::pe::{Client, Sampling};

use crate::catalogue::Target;
use crate::ctx::*;

const SAMPLING: Sampling = Sampling { temperature: 0.2, min_p: 0.0 };

fn client(cx: &Ctx, t: Target) -> Client {
    let Target::Chat(c) = t else { unreachable!("a chat server's case") };
    cx.chats[c].proc.as_ref().expect("an open chat server").client.clone()
}

fn short(s: &str) -> String {
    let s = s.trim().replace('\n', " ");
    match s.char_indices().nth(120) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s,
    }
}

pub fn answers(cx: &mut Ctx, t: Target) -> Verdict {
    if client(cx, t).ready() { pass("GET /v1/models answers") } else { fail("GET /v1/models doesn't answer 200") }
}

pub fn chat(cx: &mut Ctx, t: Target) -> Verdict {
    let c = client(cx, t);
    let at = Instant::now();
    match c.chat("Answer in one word.", "What colour is a clear daytime sky?", &[], 32, SAMPLING) {
        Ok(a) if a.trim().is_empty() => fail("answered nothing"),
        Ok(a) => pass(format!("\"{}\" in {:.1} s", short(&a), at.elapsed().as_secs_f32())),
        Err(e) => fail(e),
    }
}

pub fn stop(cx: &mut Ctx, t: Target) -> Verdict {
    let c = client(cx, t);
    let r = c.chat_stream(
        "You write long stories.",
        "Tell a long story about a lighthouse.",
        &[],
        400,
        SAMPLING,
        |n, _| n < 5,
    );
    c.abort();
    let cut = match r {
        Err(e) if e == "stopped" => "cut off after 5 pieces".to_string(),
        Ok(a) => format!("it answered at once ({} characters: not streamed)", a.len()),
        Err(e) => return fail(e),
    };
    cx.log(&cut);
    let at = Instant::now();
    match c.chat("Answer in one word.", "Say yes.", &[], 8, SAMPLING) {
        Ok(a) => pass(format!("{cut}; the next ask answered \"{}\" in {:.1} s", short(&a), at.elapsed().as_secs_f32())),
        Err(e) => fail(format!("{cut}; the next ask failed: {e}")),
    }
}

pub fn vision(cx: &mut Ctx, t: Target) -> Verdict {
    let Target::Chat(i) = t else { unreachable!() };
    let known = cx.chats[i].server.vision_known();
    let c = client(cx, t);
    let png = solid_png(256, [220, 20, 20]);
    match c.chat("Answer in one word.", "What colour is this image?", &[png], 16, SAMPLING) {
        Ok(a) if a.to_lowercase().contains("red") => pass(format!("it sees the red square: \"{}\"", short(&a))),
        Ok(a) if known == Some(true) => fail(format!("asked the colour of a red square: \"{}\"", short(&a))),
        Ok(a) => warn(format!(
            "asked the colour of a red square: \"{}\": it may not see images (no projector?): its asks would go \
             without them",
            short(&a)
        )),
        Err(e) if known == Some(true) => fail(e),
        Err(e) => warn(format!("an ask with an image failed: {e}: it may not take images")),
    }
}
