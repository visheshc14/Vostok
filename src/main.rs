use anyhow::*;
use hyper::{
    client::{connect::dns::GaiResolver, HttpConnector},
    Body, Client, Request, Response, Server, StatusCode,
};
use log::{debug, info};
use routerify::prelude::*;
use routerify::{Middleware, RequestInfo, Router, RouterService};
use std::net::SocketAddr;
use std::sync::Arc;

const LOG_LEVEL: log::LevelFilter = log::LevelFilter::Debug;
const PROXY_URL: &str = "https://httpbin.org";

struct Env {
    client: Arc<Client<hyper_rustls::HttpsConnector<HttpConnector<GaiResolver>>, hyper::Body>>,
    state: State,
}
struct State(u64);

async fn user_handler_2(req: Request<Body>) -> Result<Response<Body>> {
    let env = req.data::<Env>().unwrap();
    debug!("State value: {}", env.state.0);

    Ok(Response::new(Body::from("User 2 page!")))
}

async fn home_handler(_req: Request<Body>) -> Result<Response<Body>> {
    debug!("State value: ?");

    Ok(Response::new(Body::from("Home page")))
}

async fn user_handler(req: Request<Body>) -> Result<Response<Body>> {
    let user_id = req.param("userId").unwrap();
    Ok(Response::new(Body::from(format!("Hello {}", user_id))))
}

fn setup_logging_service() -> Result<()> {
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "{}[{}] {}",
                chrono::Utc::now().format("[%Y-%m-%d][%H:%M:%S]"),
                record.level(),
                message
            ))
        })
        .level(LOG_LEVEL)
        .chain(std::io::stdout())
        .apply()
        .context("Setting up logging service")?;
    Ok(())
}

async fn logger(req: Request<Body>) -> Result<Request<Body>> {
    debug!(
        "{} {} {}",
        req.remote_addr(),
        req.method(),
        req.uri().path()
    );
    Ok(req)
}

async fn error_handler(err: routerify::Error, _: RequestInfo) -> Response<Body> {
    eprintln!("{}", err);
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(Body::from(format!("Something went wrong: {}", err)))
        .unwrap()
}

fn router() -> Router<Body, anyhow::Error> {
    let https = hyper_rustls::HttpsConnector::with_native_roots();
    let client: Client<hyper_rustls::HttpsConnector<HttpConnector<GaiResolver>>, hyper::Body> =
        Client::builder().build(https);
    let client = Arc::new(client);

    let mut r = Router::builder().data(Env {
        client,
        state: State(100),
    });

    if LOG_LEVEL == log::LevelFilter::Debug {
        r = r.middleware(Middleware::pre(logger));
    }
    r.get("/", home_handler)
        .get("/users/:userId", user_handler)
        .get("/users/:userId/test", user_handler_2)
        .get("/*", proxy::proxy_handler)
        .err_handler_with_info(error_handler)
        .build()
        .unwrap()
}

mod proxy {
    use super::*;

    pub async fn proxy_handler(mut req: Request<Body>) -> Result<Response<Body>> {
        let env = req.data::<Env>().unwrap();
        let client = env.client.clone();
        debug!("State value: {}", env.state.0);

        rewrite_to_proxy(&mut req)?;
        client
            .request(req)
            .await
            .context("Making request to backend server")
    }

    fn rewrite_to_proxy(req: &mut Request<Body>) -> Result<()> {
        let blacklisted_headers = [
            "content-length",
            "transfer-encoding",
            "accept-encoding",
            "content-encoding",
        ];
        blacklisted_headers.iter().for_each(|key| {
            req.headers_mut().remove(*key);
        });

        let uri = req.uri();
        let uri_string = match uri.query() {
            None => format!("{}{}", PROXY_URL, uri.path()),
            Some(query) => format!("{}{}?{}", PROXY_URL, uri.path(), query),
        };
        *req.uri_mut() = uri_string
            .parse()
            .context("Parsing URI in rewrite_to_proxy")?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    setup_logging_service()?;

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

    let router = router();
    let service = RouterService::new(router).unwrap();

    let server = Server::bind(&addr).serve(service);

    info!("App is running on: {}", addr);
    info!("Try calling http://localhost:3000/uuid to test the proxy.");
    server
        .await
        .context("Fatal server error resulting in the hyper server stopping")?;
    Ok::<(), anyhow::Error>(())
}
