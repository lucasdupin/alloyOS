use metalos_forge::gguf::LoadError;
use metalos_forge::{Forge, ForgeError, Gemma2, GenerationRequest};
use std::{boxed::Box, collections::BTreeMap};
#[derive(Clone)]
enum Meta {
    Text(String),
    Uint(u32),
    Float(f32),
    Bool(bool),
    Tokens(Vec<String>),
    Scores(Vec<f32>),
}
fn string(out: &mut Vec<u8>, s: &str) {
    out.extend((s.len() as u64).to_le_bytes());
    out.extend(s.bytes());
}
fn fixture(scale: f32) -> Vec<u8> {
    let mut kv = BTreeMap::new();
    kv.insert("general.architecture", Meta::Text("gemma2".into()));
    for (key, v) in [
        ("embedding_length", 2),
        ("block_count", 2),
        ("attention.head_count", 1),
        ("attention.head_count_kv", 1),
        ("attention.key_length", 2),
        ("feed_forward_length", 2),
        ("attention.sliding_window", 2),
        ("context_length", 1024),
    ] {
        kv.insert(
            Box::leak(format!("gemma2.{key}").into_boxed_str()),
            Meta::Uint(v),
        );
    }
    for (key, v) in [
        ("attention.layer_norm_rms_epsilon", 0.000001),
        ("attn_logit_softcapping", 50.0),
        ("final_logit_softcapping", 30.0),
    ] {
        kv.insert(
            Box::leak(format!("gemma2.{key}").into_boxed_str()),
            Meta::Float(v),
        );
    }
    let mut tokens: Vec<String> = vec![
        "<unk>",
        "<bos>",
        "<eos>",
        "<start_of_turn>",
        "<end_of_turn>",
        "user",
        "model",
        "\n",
        "hi",
        "A",
        "B",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    for b in 0..256 {
        tokens.push(format!("<0x{b:02X}>"));
    }
    let scores = vec![0.0; tokens.len()];
    let vocab = tokens.len();
    kv.insert("tokenizer.ggml.model", Meta::Text("llama".into()));
    kv.insert("tokenizer.ggml.bos_token_id", Meta::Uint(1));
    kv.insert("tokenizer.ggml.eos_token_id", Meta::Uint(2));
    kv.insert("tokenizer.ggml.tokens", Meta::Tokens(tokens));
    kv.insert("tokenizer.ggml.scores", Meta::Scores(scores));
    kv.insert("tokenizer.ggml.add_space_prefix", Meta::Bool(false));
    let mut tensors = Vec::new();
    let mut embedding = vec![0.01; vocab * 2];
    embedding[9 * 2] = scale;
    embedding[9 * 2 + 1] = 0.0;
    embedding[10 * 2] = -scale;
    embedding[10 * 2 + 1] = 0.0;
    tensors.push(("token_embd.weight".into(), vec![2, vocab], embedding));
    tensors.push(("output_norm.weight".into(), vec![2], vec![1., 1.]));
    for l in 0..2 {
        for name in [
            "attn_norm",
            "post_attention_norm",
            "ffn_norm",
            "post_ffw_norm",
        ] {
            tensors.push((format!("blk.{l}.{name}.weight"), vec![2], vec![1., 1.]));
        }
        for name in [
            "attn_q",
            "attn_k",
            "attn_v",
            "attn_output",
            "ffn_gate",
            "ffn_up",
            "ffn_down",
        ] {
            tensors.push((
                format!("blk.{l}.{name}.weight"),
                vec![2, 2],
                vec![0.1, 0.0, 0.0, 0.1],
            ));
        }
    }
    let mut out = b"GGUF".to_vec();
    out.extend(3u32.to_le_bytes());
    out.extend((tensors.len() as u64).to_le_bytes());
    out.extend((kv.len() as u64).to_le_bytes());
    for (key, v) in kv {
        string(&mut out, key);
        match v {
            Meta::Text(s) => {
                out.extend(8u32.to_le_bytes());
                string(&mut out, &s)
            }
            Meta::Uint(v) => {
                out.extend(4u32.to_le_bytes());
                out.extend(v.to_le_bytes())
            }
            Meta::Float(v) => {
                out.extend(6u32.to_le_bytes());
                out.extend(v.to_le_bytes())
            }
            Meta::Bool(v) => {
                out.extend(7u32.to_le_bytes());
                out.push(v as u8)
            }
            Meta::Tokens(tokens) => {
                out.extend(9u32.to_le_bytes());
                out.extend(8u32.to_le_bytes());
                out.extend((tokens.len() as u64).to_le_bytes());
                for t in tokens {
                    string(&mut out, &t)
                }
            }
            Meta::Scores(scores) => {
                out.extend(9u32.to_le_bytes());
                out.extend(6u32.to_le_bytes());
                out.extend((scores.len() as u64).to_le_bytes());
                for s in scores {
                    out.extend(s.to_le_bytes())
                }
            }
        }
    }
    let mut payload = Vec::new();
    for (name, shape, values) in tensors {
        while payload.len() % 32 != 0 {
            payload.push(0)
        }
        string(&mut out, &name);
        out.extend((shape.len() as u32).to_le_bytes());
        for d in shape {
            out.extend((d as u64).to_le_bytes())
        }
        out.extend(0u32.to_le_bytes());
        out.extend((payload.len() as u64).to_le_bytes());
        for v in values {
            payload.extend(v.to_le_bytes())
        }
    }
    while out.len() % 32 != 0 {
        out.push(0)
    }
    out.extend(payload);
    out
}
#[test]
fn weight_driven_generation_and_resident_multi_model_selection() {
    let a = Box::leak(fixture(2.0).into_boxed_slice());
    let b = Box::leak(fixture(-2.0).into_boxed_slice());
    let mut forge = Forge::new(1 << 20);
    forge.register("fixture-a").unwrap();
    forge.register("fixture-b").unwrap();
    forge
        .attach("fixture-a", Box::new(Gemma2::load("fixture-a", a).unwrap()))
        .unwrap();
    forge
        .attach("fixture-b", Box::new(Gemma2::load("fixture-b", b).unwrap()))
        .unwrap();
    let r = GenerationRequest {
        model: None,
        prompt: "hi".into(),
        max_tokens: 2,
    };
    forge.set_default("fixture-a").unwrap();
    let first = forge.generate(&r).unwrap();
    assert_eq!(first.text, "AA");
    forge.set_default("fixture-b").unwrap();
    let second = forge.generate(&r).unwrap();
    assert_eq!(second.text, "BB");
    assert_ne!(first, second);
    assert_eq!(forge.generate(&r).unwrap(), second);
    assert_eq!(
        forge.models().iter().filter(|m| m.state == "ready").count(),
        2
    );
}
#[test]
fn rejects_truncated_weights_and_budget() {
    let mut bytes = fixture(2.);
    bytes.pop();
    assert!(matches!(
        Gemma2::load("fixture", Box::leak(bytes.into_boxed_slice())),
        Err(LoadError::Truncated)
    ));
    let model = Gemma2::load("fixture", Box::leak(fixture(2.).into_boxed_slice())).unwrap();
    let mut f = Forge::new(0);
    f.register("fixture").unwrap();
    assert_eq!(
        f.attach("fixture", Box::new(model)),
        Err(ForgeError::MemoryBudgetExceeded)
    );
    assert_eq!(
        f.models().iter().find(|m| m.id == "fixture").unwrap().state,
        "weights_missing"
    );
}

#[test]
fn load_diagnostics_are_machine_readable_and_clear_on_attach() {
    use metalos_forge::protocol::{decode_hex, Request};
    let mut forge = Forge::new(1 << 20);
    forge.register("fixture").unwrap();
    forge
        .mark_load_error("fixture", &LoadError::UnsupportedEncoding(14))
        .unwrap();
    let reply = forge.dispatch(Request::parse("FORGE1\t1\tstatus").unwrap());
    let hex = reply.trim_end().split('\t').nth(3).unwrap();
    assert!(decode_hex(hex)
        .unwrap()
        .contains("\"error\":\"unsupported_tensor_encoding\""));
    forge
        .attach(
            "fixture",
            Box::new(Gemma2::load("fixture", Box::leak(fixture(2.).into_boxed_slice())).unwrap()),
        )
        .unwrap();
    assert_eq!(
        forge
            .models()
            .iter()
            .find(|m| m.id == "fixture")
            .unwrap()
            .error,
        None
    );
}

#[test]
fn local_chat_starts_without_computing_and_yields_through_prefill_and_streaming() {
    use core::task::Poll;
    use metalos_forge::InferenceBackend;
    let weights = Box::leak(fixture(2.).into_boxed_slice());
    let request = GenerationRequest {
        model: Some("fixture".into()),
        prompt: "a".into(),
        max_tokens: 3,
    };
    let mut blocking = Gemma2::load("fixture", weights).unwrap();
    let expected = blocking.generate(&request).unwrap();
    let mut cooperative = Gemma2::load("fixture", weights).unwrap();
    assert!(cooperative.start_generation(&request).unwrap().is_none());
    assert!(matches!(
        cooperative.start_generation(&request),
        Err(ForgeError::Busy)
    ));
    let mut ticks = 0;
    let mut saw_partial = false;
    loop {
        match cooperative.poll_generation() {
            Poll::Pending => {
                ticks += 1;
                saw_partial |= cooperative.partial_text().is_some();
                // Other UI work has a chance to run after every bounded poll.
                assert!(ticks < 1000, "local reply never completed");
            }
            Poll::Ready(result) => {
                assert_eq!(result.unwrap(), expected);
                break;
            }
        }
    }
    assert!(ticks > 0, "reply never yielded its streamed text");
    assert!(
        saw_partial,
        "text stayed hidden until the entire reply completed"
    );
}
