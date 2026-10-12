use core::task::Poll;
use metalos_forge::{
    cloud::{ApiProvider, HttpRequest, HttpResponse, HttpsClient},
    Forge, ForgeError,
};
use metalos_welder::AgentRuntime;
use std::{cell::RefCell, rc::Rc};

struct DelayedReply(usize);
impl HttpsClient for DelayedReply {
    fn post(&mut self, _: &HttpRequest) -> Result<HttpResponse, ForgeError> {
        panic!("UI conversation reached a blocking transport");
    }
    fn start_post(&mut self, _: &HttpRequest) -> Poll<Result<HttpResponse, ForgeError>> {
        Poll::Pending
    }
    fn poll_post(&mut self) -> Poll<Result<HttpResponse, ForgeError>> {
        self.0 += 1;
        if self.0 < 120 {
            return Poll::Pending;
        }
        Poll::Ready(Ok(HttpResponse {
            status: 200,
            body: r#"{"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Ready"}]}],"usage":{"output_tokens":1}}"#.into(),
        }))
    }
}

#[test]
fn ui_conversation_yields_and_delivers_a_reply_once() {
    let forge = Rc::new(RefCell::new(Forge::new(0)));
    forge
        .borrow_mut()
        .connect_api(
            ApiProvider::OpenAi,
            ApiProvider::OpenAi.default_model(),
            "fixture",
            Box::new(DelayedReply(0)),
        )
        .unwrap();
    let mut agent = AgentRuntime::new();
    agent.set_forge(forge);
    agent.switch_provider("openai").unwrap();
    assert!(agent.start_console_input("Hello").unwrap().is_none());
    assert!(agent.start_console_input("Duplicate").is_err());
    for _ in 0..119 {
        assert!(agent.poll_console_reply().is_pending());
    }
    let Poll::Ready(Ok(reply)) = agent.poll_console_reply() else {
        panic!("missing reply");
    };
    assert_eq!(reply.response_text, "Ready");
    assert!(agent.poll_console_reply().is_pending());
}
