//! WebSocket 实时行情推送。
use crate::state::{AppState, QuotePushEvent};
use actix::{Actor, ActorContext, AsyncContext, StreamHandler};
use actix_web::{web, Error, HttpRequest, HttpResponse};
use actix_web_actors::ws;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

struct QuoteSocket {
    receiver: broadcast::Receiver<QuotePushEvent>,
    last_pong: Instant,
}

impl Actor for QuoteSocket {
    type Context = ws::WebsocketContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.run_interval(Duration::from_millis(200), |actor, ctx| loop {
            match actor.receiver.try_recv() {
                Ok(event) => {
                    if let Ok(text) = serde_json::to_string(&event) {
                        ctx.text(text);
                    }
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Closed) => {
                    ctx.close(None);
                    ctx.stop();
                    break;
                }
            }
        });
        ctx.run_interval(Duration::from_secs(15), |actor, ctx| {
            if actor.last_pong.elapsed() > Duration::from_secs(45) {
                ctx.close(None);
                ctx.stop();
            } else {
                ctx.ping(b"mojin");
            }
        });
    }
}

impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for QuoteSocket {
    fn handle(&mut self, message: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
        match message {
            Ok(ws::Message::Ping(bytes)) => ctx.pong(&bytes),
            Ok(ws::Message::Pong(_)) => self.last_pong = Instant::now(),
            Ok(ws::Message::Close(reason)) => {
                ctx.close(reason);
                ctx.stop();
            }
            Err(_) => ctx.stop(),
            _ => {}
        }
    }
}

/// 连接后服务端推送 `{ "type": "quote", "quote": {...} }`。
pub async fn quote_ws(
    request: HttpRequest,
    stream: web::Payload,
    state: web::Data<AppState>,
) -> Result<HttpResponse, Error> {
    ws::start(
        QuoteSocket {
            receiver: state.quote_hub.subscribe(),
            last_pong: Instant::now(),
        },
        &request,
        stream,
    )
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/ws/quote", web::get().to(quote_ws));
}
