use crate::h264_depacketizer::H264Depacketizer;
use crate::node_shadow::{NodeShadow, NodeShadows};
use crate::rtsp::RtspClient;
use anyhow::{anyhow, Result};
use orb_node::{Auth, Service};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::interceptor::registry::Registry;
use webrtc::media::Sample;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;

pub struct WebRtcBridge {
    // service_id -> StreamSession
    sessions: Arc<Mutex<HashMap<String, Arc<StreamSession>>>>,
    // Known services from config or announced by nodes
    node_shadows: Arc<Mutex<NodeShadows>>,
}

pub struct StreamSession {
    pub service_id: String,
    pub track: Arc<TrackLocalStaticSample>,
    pub depacketizer: Mutex<H264Depacketizer>,
    pub last_timestamp: Mutex<Option<u32>>,
    pub subscriber_count: Arc<AtomicUsize>,
    pub rtp_tx: mpsc::Sender<Vec<u8>>,
}

pub type NodeId = String;

impl WebRtcBridge {
    pub fn default() -> Self {
        Self::new(HashMap::new())
    }

    pub fn new(node_shadows: NodeShadows) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            // Update to use HashMap<node_id, Vec<Service>> if needed
            node_shadows: Arc::new(Mutex::new(node_shadows)),
        }
    }

    async fn find_service(&self, service_id: &str) -> Option<Service> {
        let node_shadows = self.node_shadows.lock().await;
        for (_node_id, shadow) in node_shadows.iter() {
            for svc in &shadow.services {
                if svc.id == service_id {
                    return Some(svc.clone());
                }
            }
        }
        None
    }

    pub async fn create_session(&self, service_id: &str) -> Result<Arc<StreamSession>> {
        let mut sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get(service_id) {
            return Ok(Arc::clone(session));
        }

        let service = self
            .find_service(service_id)
            .await
            .ok_or_else(|| anyhow!("Service not found: {}", service_id))?;

        if service.svc_type != "rtsp" {
            return Err(anyhow!("Unsupported service type: {}", service.svc_type));
        }

        // Create RTP channel for this session
        let (rtp_tx, mut rtp_rx) = mpsc::channel::<Vec<u8>>(100);

        let track = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: "video/h264".to_owned(),
                ..Default::default()
            },
            "video".to_owned(),
            "orb-webrtc".to_owned(),
        ));

        let depacketizer = H264Depacketizer::new();

        let session = Arc::new(StreamSession {
            service_id: service_id.to_string(),
            track,
            depacketizer: Mutex::new(depacketizer),
            last_timestamp: Mutex::new(None),
            subscriber_count: Arc::new(AtomicUsize::new(0)),
            rtp_tx,
        });

        // Spawn RTP ingestion task
        let session_clone = Arc::clone(&session);
        tokio::spawn(async move {
            while let Some(data) = rtp_rx.recv().await {
                if let Err(e) = session_clone.ingest_rtp(data).await {
                    eprintln!("Error ingesting RTP: {}", e);
                }
            }
        });

        sessions.insert(service_id.to_string(), Arc::clone(&session));
        Ok(session)
    }

    pub async fn start_rtsp_client(&self, service_id: &str) -> Result<()> {
        let service = self
            .find_service(service_id)
            .await
            .ok_or_else(|| anyhow!("Service not found: {}", service_id))?;

        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(service_id)
            .ok_or_else(|| anyhow!("Session not found: {}", service_id))?;

        let rtp_tx = session.rtp_tx.clone();
        let service_id = service_id.to_string();

        tokio::spawn(async move {
            let path = if service.path.starts_with('/') {
                service.path.clone()
            } else {
                format!("/{}", service.path)
            };

            println!(
                "Connecting to RTSP service {} at {}:{}{}",
                service_id, service.addr, service.port, path
            );

            let mut rtsp_client = RtspClient::new(
                service.addr.clone(),
                service.port,
                path.clone(),
                service.auth.as_ref().and_then(|auth| match auth {
                    Auth::UsernameAndPassword { username, password } => {
                        Some((username.clone(), password.clone()))
                    }
                }),
            );

            match rtsp_client.connect_and_stream(rtp_tx).await {
                Ok(_) => println!("RTSP stream ended for {}", service_id),
                Err(e) => eprintln!("Failed to connect to RTSP service {}: {}", service_id, e),
            }
        });

        Ok(())
    }

    pub async fn handle_offer(&self, sdp: String, service_id: String) -> Result<String> {
        let session = self.create_session(&service_id).await?;

        // Check if this is the first subscriber (need to start RTSP client)
        let is_first = session.subscriber_count.fetch_add(1, Ordering::SeqCst) == 0;
        if is_first {
            self.start_rtsp_client(&service_id).await?;
        }

        // ... (middleware setup) ...
        let mut m = MediaEngine::default();
        m.register_default_codecs()?;

        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut m)?;

        let api = APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .build();

        let config = RTCConfiguration {
            ..Default::default()
        };

        let pc = Arc::new(api.new_peer_connection(config).await?);

        pc.add_track(Arc::clone(&session.track) as Arc<dyn TrackLocal + Send + Sync>)
            .await?;

        // ... (ICE handler setup) ...
        let sessions_map_clone = Arc::clone(&self.sessions);
        let service_id_clone = service_id.clone();

        let pc_clone = Arc::clone(&pc);
        pc.on_ice_connection_state_change(Box::new(move |state: RTCIceConnectionState| {
            println!("ICE Connection State has changed: {}", state);
            let sessions_map_clone = sessions_map_clone.clone();
            let service_id_clone = service_id_clone.clone();
            let pc_clone = Arc::clone(&pc_clone);

            Box::pin(async move {
                if state == RTCIceConnectionState::Disconnected
                    || state == RTCIceConnectionState::Failed
                {
                    // Start a timeout to clean up if the connection doesn't recover
                    tokio::time::sleep(Duration::from_secs(5)).await;

                    // Re-check state after timeout
                    if pc_clone.ice_connection_state() == RTCIceConnectionState::Disconnected
                        || pc_clone.ice_connection_state() == RTCIceConnectionState::Failed
                    {
                        let mut sessions = sessions_map_clone.lock().await;
                        if let Some(session) = sessions.get(&service_id_clone) {
                            let count = session.subscriber_count.fetch_sub(1, Ordering::SeqCst);
                            if count == 1 {
                                println!(
                                    "Last subscriber disconnected for {}. Cleaning up.",
                                    service_id_clone
                                );
                                sessions.remove(&service_id_clone);
                            }
                        }
                    }
                } else if state == RTCIceConnectionState::Closed {
                    let mut sessions = sessions_map_clone.lock().await;
                    if let Some(session) = sessions.get(&service_id_clone) {
                        let count = session.subscriber_count.fetch_sub(1, Ordering::SeqCst);
                        if count == 1 {
                            println!(
                                "Last subscriber disconnected for {}. Cleaning up.",
                                service_id_clone
                            );
                            sessions.remove(&service_id_clone);
                        }
                    }
                }
            })
        }));

        let mut remote_desc = RTCSessionDescription::default();
        remote_desc.sdp = sdp;
        remote_desc.sdp_type = webrtc::peer_connection::sdp::sdp_type::RTCSdpType::Offer;

        pc.set_remote_description(remote_desc).await?;

        let answer = pc.create_answer(None).await?;
        pc.set_local_description(answer.clone()).await?;

        let mut gather_complete = pc.gathering_complete_promise().await;
        let _ = gather_complete.recv().await;

        let local_desc = pc
            .local_description()
            .await
            .ok_or_else(|| anyhow!("Failed to get local description"))?;

        Ok(local_desc.sdp)
    }

    /// Register services announced by a node
    pub async fn register_services(&self, node_id: &str, services: Vec<Service>) -> Result<()> {
        let mut self_node_shadows = self.node_shadows.lock().await;
        // Insert or replace services for this node
        self_node_shadows.insert(node_id.to_string(), NodeShadow { services });
        Ok(())
    }

    /// Remove all services from a node (when it disconnects)
    pub async fn unregister_node(&self, node_id: &str) {
        let mut self_node_shadows = self.node_shadows.lock().await;
        self_node_shadows.remove(node_id);
    }
}

impl StreamSession {
    async fn ingest_rtp(&self, data: Vec<u8>) -> Result<()> {
        let mut depacketizer = self.depacketizer.lock().await;

        // Detect if data is RTSP-framed ($) or raw RTP
        let rtp_packet = if !data.is_empty() && data[0] == b'$' {
            if data.len() < 4 {
                return Ok(());
            }
            let channel = data[1];
            // Filter RTCP channels (typically odd: 1, 3, etc.)
            if channel % 2 != 0 {
                return Ok(());
            }
            &data[4..]
        } else {
            &data[..]
        };

        // Basic RTP parsing
        if rtp_packet.len() < 12 {
            return Ok(());
        }

        let marker = (rtp_packet[1] & 0x80) != 0;
        let timestamp =
            u32::from_be_bytes([rtp_packet[4], rtp_packet[5], rtp_packet[6], rtp_packet[7]]);
        let payload = &rtp_packet[12..];

        if let Some(frame) = depacketizer.push(payload)? {
            // Calculate duration based on RTP timestamp diff
            let mut last_ts_guard = self.last_timestamp.lock().await;
            let duration = if let Some(last_ts) = *last_ts_guard {
                if marker {
                    let diff = timestamp.wrapping_sub(last_ts);
                    *last_ts_guard = Some(timestamp);
                    // H.264 RTP timestamp clock is usually 90000Hz
                    if diff > 0 && diff < 90000 {
                        Duration::from_nanos((diff as u64 * 1_000_000_000) / 90000)
                    } else {
                        // Default to 33ms if timestamp diff is suspicious or 0
                        Duration::from_millis(33)
                    }
                } else {
                    // Not the end of a frame, use 0 duration to keep same timestamp
                    Duration::from_millis(0)
                }
            } else {
                *last_ts_guard = Some(timestamp);
                Duration::from_millis(0)
            };

            // Prepend Annex B start code if it's not a STAP-A
            let nalu_type = frame.data[0] & 0x1F;
            let final_data = if nalu_type == 24 {
                frame.data.into()
            } else {
                let mut d = Vec::with_capacity(frame.data.len() + 4);
                d.extend_from_slice(&[0, 0, 0, 1]);
                d.extend_from_slice(&frame.data);
                d.into()
            };

            self.track
                .write_sample(&Sample {
                    data: final_data,
                    duration,
                    ..Default::default()
                })
                .await?;
        }

        Ok(())
    }
}
