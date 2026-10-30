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
