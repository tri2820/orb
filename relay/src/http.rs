use crate::webrtc::WebRtcBridge;
use std::sync::Arc;
use warp::Filter;

#[derive(serde::Deserialize)]
pub struct OfferRequest {
    sdp: String,
    #[serde(rename = "serviceId")]
    service_id: String,
}

#[derive(serde::Serialize)]
pub struct AnswerResponse {
    sdp: String,
    #[serde(rename = "type")]
    sdp_type: String,
}

pub fn routes(
    webrtc_bridge: Arc<WebRtcBridge>,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    let offer_route = warp::post()
        .and(warp::path("offer"))
        .and(warp::body::bytes())
        .and_then(move |body: bytes::Bytes| {
            let bridge = webrtc_bridge.clone();
            async move {
                let req: OfferRequest = match serde_json::from_slice(&body) {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("JSON deserialization error: {}", e);
                        return Err(warp::reject::reject());
                    }
                };

                match bridge.handle_offer(req.sdp, req.service_id).await {
                    Ok(answer_sdp) => {
                        Ok::<_, warp::Rejection>(warp::reply::json(&AnswerResponse {
                            sdp: answer_sdp,
                            sdp_type: "answer".to_string(),
                        }))
                    }
                    Err(e) => {
                        eprintln!("WebRTC offer error: {}", e);
                        Err(warp::reject::reject())
                    }
                }
            }
        });

    offer_route
}
