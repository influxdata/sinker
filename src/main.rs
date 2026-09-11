#![deny(rustdoc::broken_intra_doc_links, rustdoc::bare_urls, rust_2018_idioms)]

use clap::{Parser, Subcommand};
use kube::CustomResourceExt;

use sinker::controller;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    #[derive(Clone, Parser)]
    #[clap(version)]
    struct Args {
        /// The tracing filter used for logs
        #[clap(long, env = "SINKER_LOG", default_value = "sinker=info,warn")]
        log_level: kubert::LogFilter,

        /// The logging format
        #[clap(long, default_value = "plain")]
        log_format: kubert::LogFormat,

        #[clap(flatten)]
        client: kubert::ClientArgs,

        #[clap(flatten)]
        admin: kubert::AdminArgs,

        #[command(subcommand)]
        command: Option<Commands>,
    }

    #[derive(Clone, Subcommand)]
    enum Commands {
        /// Generates k8s manifests
        Manifests,
    }

    let Args {
        log_level,
        log_format,
        client,
        admin,
        command,
    } = Args::parse();

    match &command {
        Some(Commands::Manifests) => {
            println!(
                "{}---\n{}",
                serde_yaml::to_string(&sinker::resources::ResourceSync::crd())?,
                serde_yaml::to_string(
                    &sinker::resources::SinkerContainer::crd_with_manual_schema()
                )?
            );
        }
        None => {
            let rt = kubert::Runtime::builder()
                .with_log(log_level, log_format)
                .with_admin(admin)
                .with_client(client)
                .build()
                .await?;

            let controller = controller::run(controller_client(rt.client()));

            // Both runtimes implements graceful shutdown, so poll until both are done
            tokio::join!(controller, rt.run()).1?;
        }
    }

    Ok(())
}

/// Reuse Kubert's configured transport until it supports the current kube release.
/// This preserves its credentials, impersonation, timeouts, and default namespace.
fn controller_client(client: kubert::client::Client) -> kube::Client {
    let namespace = client.default_namespace().to_owned();
    let service = tower::service_fn(move |request: http::Request<kube::client::Body>| {
        let client = client.clone();
        async move {
            let (parts, body) = request.into_parts();
            // Sinker sends finite JSON request bodies; keep watch responses streaming.
            let body = body.collect_bytes().await?;
            client
                .send(http::Request::from_parts(parts, body.into()))
                .await
                .map_err(|error| match error {
                    kubert::client::Error::Service(source) => kube::Error::Service(source),
                    kubert::client::Error::HyperError(source) => kube::Error::HyperError(source),
                    error => kube::Error::Service(Box::new(error)),
                })
        }
    });
    kube::Client::new(service, namespace)
}

#[cfg(test)]
mod tests {
    use super::controller_client;
    use futures::{stream, StreamExt};
    use http::{Request, Response, StatusCode};
    use http_body::Frame;
    use http_body_util::{BodyExt, StreamBody};
    use kube::client::Body;
    use kubert::client::{client::Body as LegacyBody, Client as LegacyClient};
    use rstest::rstest;
    use std::{convert::Infallible, time::Duration};
    use tokio::time::timeout;

    #[rstest]
    #[case::get("GET", "", StatusCode::OK)]
    #[case::patch("PATCH", r#"{"data":{"key":"value"}}"#, StatusCode::CREATED)]
    #[case::api_error("GET", "", StatusCode::FORBIDDEN)]
    #[tokio::test]
    async fn controller_client_preserves_http_exchange(
        #[case] method: &'static str,
        #[case] payload: &'static str,
        #[case] status: StatusCode,
    ) {
        let uri = "/api/v1/namespaces/workloads/configmaps/target?fieldManager=sinker.influxdata.io&force=true";
        let service = tower::service_fn(move |request: Request<LegacyBody>| async move {
            assert_eq!(request.method(), method);
            assert_eq!(request.uri(), uri);
            assert_eq!(
                request.headers()["content-type"],
                "application/apply-patch+yaml"
            );
            assert_eq!(request.extensions().get::<u32>(), Some(&42));
            assert_eq!(
                request
                    .into_body()
                    .collect_bytes()
                    .await
                    .expect("request body"),
                payload
            );
            Ok::<_, Infallible>(
                Response::builder()
                    .status(status)
                    .header("x-test-response", "preserved")
                    .body(LegacyBody::from(b"response body".to_vec()))
                    .expect("response"),
            )
        });
        let client = controller_client(LegacyClient::new(service, "workloads"));
        assert_eq!(client.default_namespace(), "workloads");
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/apply-patch+yaml")
            .extension(42_u32)
            .body(Body::from(payload.as_bytes().to_vec()))
            .expect("request");
        let response = timeout(Duration::from_secs(5), client.send(request))
            .await
            .expect("request completes")
            .expect("send request");
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()["x-test-response"], "preserved");
        assert_eq!(
            response
                .into_body()
                .collect_bytes()
                .await
                .expect("response body"),
            "response body"
        );
    }

    #[tokio::test]
    async fn controller_client_keeps_watch_responses_streaming() {
        let service = tower::service_fn(|_: Request<LegacyBody>| async {
            let frames = stream::iter([Ok::<_, Infallible>(Frame::data("watch event\n".into()))])
                .chain(stream::pending());
            Ok::<_, Infallible>(Response::new(StreamBody::new(frames)))
        });
        let client = controller_client(LegacyClient::new(service, "default"));
        let request = Request::builder()
            .uri("/api/v1/pods?watch=true")
            .body(Body::empty())
            .expect("watch request");
        let response = timeout(Duration::from_secs(5), client.send(request))
            .await
            .expect("headers arrive before the watch ends")
            .expect("watch response");
        let mut body = response.into_body();
        let frame = timeout(Duration::from_secs(5), body.frame())
            .await
            .expect("event arrives before the watch ends")
            .expect("event frame")
            .expect("read frame");
        assert_eq!(frame.into_data().expect("data frame"), "watch event\n");
    }

    #[tokio::test]
    async fn controller_client_preserves_api_error_details() {
        let service = tower::service_fn(|_: Request<LegacyBody>| async {
            Ok::<_, Infallible>(Response::builder().status(StatusCode::FORBIDDEN)
                .body(LegacyBody::from(br#"{"apiVersion":"v1","kind":"Status","status":"Failure","message":"access denied","reason":"Forbidden","code":403}"#.to_vec()))
                .expect("error response"))
        });
        let client = controller_client(LegacyClient::new(service, "default"));
        let error = client
            .request::<serde_json::Value>(
                Request::builder()
                    .uri("/api/v1/pods")
                    .body(Vec::new())
                    .expect("request"),
            )
            .await
            .expect_err("forbidden request");
        match error {
            kube::Error::Api(status) => {
                assert_eq!(status.code, 403);
                assert_eq!(status.reason, "Forbidden");
                assert_eq!(status.message, "access denied");
            }
            error => panic!("expected API error, got {error:?}"),
        }
    }

    #[tokio::test]
    async fn controller_client_retains_transport_error_source() {
        let service = tower::service_fn(|_: Request<LegacyBody>| async {
            Err::<Response<LegacyBody>, _>(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "connection reset by test peer",
            ))
        });
        let client = controller_client(LegacyClient::new(service, "default"));
        let error = client
            .send(Request::new(Body::empty()))
            .await
            .expect_err("transport failure");
        assert_eq!(
            error.to_string(),
            "ServiceError: connection reset by test peer"
        );
        let kube::Error::Service(source) = error else {
            panic!("expected service error, got {error:?}");
        };
        let cause = source
            .downcast_ref::<std::io::Error>()
            .expect("transport cause");
        assert_eq!(cause.kind(), std::io::ErrorKind::ConnectionReset);
        assert_eq!(cause.to_string(), "connection reset by test peer");
    }
}
