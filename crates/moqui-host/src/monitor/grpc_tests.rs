use super::*;

#[test]
fn receiver_refusals_are_the_matching_grpc_status() {
    for (code, expected) in [
        (401, tonic::Code::Unauthenticated),
        (429, tonic::Code::ResourceExhausted),
        (413, tonic::Code::ResourceExhausted),
        (503, tonic::Code::Unavailable),
        (400, tonic::Code::InvalidArgument),
        (404, tonic::Code::Unimplemented),
        (500, tonic::Code::Internal),
    ] {
        let status = status((code, format!("refused {code}")));
        assert_eq!(status.code(), expected, "{code}");
        assert_eq!(status.message(), format!("refused {code}"));
    }
}

#[test]
fn the_key_comes_from_api_key_or_a_bearer_authorization() {
    let mut metadata = tonic::metadata::MetadataMap::new();
    assert_eq!(token(&metadata), "");
    metadata.insert("authorization", "Basic abc".parse().unwrap());
    assert_eq!(token(&metadata), "");
    metadata.insert("authorization", "Bearer mqi_one".parse().unwrap());
    assert_eq!(token(&metadata), "mqi_one");
    metadata.insert("api_key", "mqi_two".parse().unwrap());
    assert_eq!(token(&metadata), "mqi_two");
}

/// An inner service that must not be reached: the key is checked before the message is read.
#[derive(Clone)]
struct Unreached;

impl tonic::server::NamedService for Unreached {
    const NAME: &'static str = "test.Unreached";
}

impl tonic::codegen::Service<http::Request<tonic::body::Body>> for Unreached {
    type Response = http::Response<tonic::body::Body>;
    type Error = std::convert::Infallible;
    type Future = tonic::codegen::BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(&mut self, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, _request: http::Request<tonic::body::Body>) -> Self::Future {
        panic!("the message was read before the key was checked");
    }
}

/// SEC-TLS-17: a request with no key is refused by `KeyFirst` before the service that decodes its message is called,
/// as the matching gRPC status in the response's headers; the service keeps its name for the router.
#[test]
fn a_request_without_a_key_is_refused_before_its_message_is_read() {
    use tonic::codegen::Service as _;
    let context = moqui_core::context::ExecutionContextFactory::test_context();
    let mut service = KeyFirst::new(Unreached, context.factory());
    assert_eq!(
        <KeyFirst<Unreached> as tonic::server::NamedService>::NAME,
        "test.Unreached"
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let response = runtime
        .block_on(service.call(http::Request::new(tonic::body::Body::empty())))
        .unwrap();
    let status = tonic::Status::from_header_map(response.headers()).expect("a gRPC status");
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
}
