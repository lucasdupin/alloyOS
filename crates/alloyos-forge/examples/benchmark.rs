//! Measure the resident backend independently of the guest and UI.
//! cargo run --release -p alloyos-forge --example benchmark -- MODEL.img [PROMPT]
use alloyos_forge::{gguf::Gguf, tokenizer::Tokenizer, Gemma2, GenerationRequest, InferenceBackend};
use std::{fs::File, os::fd::AsRawFd, task::Poll, time::Instant};

unsafe extern "C" {
    fn mmap(address: *mut core::ffi::c_void, length: usize, protection: i32,
        flags: i32, fd: i32, offset: i64) -> *mut core::ffi::c_void;
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("supply a GGUF file or model image");
    let prompt = args.next().unwrap_or_else(|| "Hello! Reply in one short sentence.".into());
    let repetitions = args.next().map(|n| n.parse::<usize>().expect("repetition count")).unwrap_or(1);
    let file = File::open(path).expect("open model");
    let length = usize::try_from(file.metadata().unwrap().len()).unwrap();
    // A read-only mapping avoids allocating a second multi-gigabyte weight copy.
    // Intentionally retain it until process exit: the backend borrows static weights.
    let mapped = unsafe { mmap(core::ptr::null_mut(), length, 1, 2, file.as_raw_fd(), 0) };
    assert_ne!(mapped as isize, -1, "mmap failed");
    let bytes: &'static [u8] = unsafe { core::slice::from_raw_parts(mapped.cast(), length) };
    let weights = if bytes.starts_with(b"GGUF") { bytes } else {
        assert!(bytes.get(4096..4100) == Some(b"GGUF"), "not a GGUF or model image");
        &bytes[4096..]
    };
    let mut gguf = Gguf::parse(weights).expect("parse GGUF");
    let tokenizer = Tokenizer::load_owned(&mut gguf).expect("tokenizer");
    println!("prompt_tokens={} weight_bytes={}", tokenizer.chat(&prompt).unwrap().len(), weights.len());
    drop(tokenizer);
    drop(gguf);
    let loading = Instant::now();
    let mut model = Gemma2::load("benchmark", weights).expect("load Gemma");
    println!("load_seconds={:.3}", loading.elapsed().as_secs_f64());
    for repetition in 0..repetitions {
    println!("repetition={}", repetition + 1);
    let started = Instant::now();
    model.start_generation(&GenerationRequest {
        model: None, prompt: prompt.clone(), max_tokens: 8,
    }).expect("start generation");
    let mut first = None;
    let mut last_text = String::new();
    loop {
        let result = model.poll_generation();
        if let Some(text) = model.partial_text() {
            if text != last_text {
                let elapsed = started.elapsed();
                first.get_or_insert(elapsed);
                println!("text_seconds={:.3} text={text:?}", elapsed.as_secs_f64());
                last_text = text.to_owned();
            }
        }
        if let Poll::Ready(result) = result {
            let output = result.expect("generation");
            let total = started.elapsed().as_secs_f64();
            let decode = total - first.unwrap_or_default().as_secs_f64();
            println!("total_seconds={total:.3} generated_tokens={} decode_tokens_per_second={:.3}",
                output.generated_tokens,
                output.generated_tokens.saturating_sub(1) as f64 / decode.max(0.000001));
            break;
        }
    }
    }
}
