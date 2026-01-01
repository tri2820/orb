# MJPEG Transcoding Support

## Overview

The relay now supports MJPEG HTTP streams in addition to RTSP streams. MJPEG streams are automatically transcoded to H.264 using GStreamer before being forwarded to WebRTC clients.

## How It Works

```
MJPEG HTTP Stream → GStreamer Transcoder → H.264 RTP → WebRTC Track → Browser
                    ├─ souphttpsrc (HTTP client)
                    ├─ multipartdemux (split MJPEG)
                    ├─ jpegdec (decode JPEG)
                    ├─ clocksync (assign timestamps) ⭐ CRITICAL
                    ├─ videorate (enforce framerate) ⭐ CRITICAL
                    ├─ videoconvert (convert to I420)
                    ├─ x264enc (encode H.264)
                    └─ rtph264pay (RTP packetizer)
```

## Architecture

### Service Type Detection

The relay automatically detects the service type based on the `svc_type` field in the Service struct:

- **`"rtsp"`** → Direct RTP forwarding (no transcoding)
- **`"mjpeg"`** → GStreamer transcoding pipeline

### Code Flow

1. **Client requests stream** via `/offer` endpoint with `serviceId`
2. **WebRtcBridge.create_session()** creates a streaming session
3. **WebRtcBridge.start_rtsp_client()** checks service type:
   - If MJPEG → Creates `MjpegTranscoder` with HTTP URL
   - If RTSP → Uses existing `RtspClient` with bridge connection
4. **Transcoder** decodes MJPEG and encodes to H.264 RTP
5. **RTP packets** forwarded to WebRTC track via same `ingest_rtp()` path

## Testing with the Axis Camera

### 1. Configure the Service

Create a service with type `"mjpeg"`:

```json
{
  "id": "axis-mjpeg",
  "type": "mjpeg",
  "addr": "83.48.75.113",
  "port": 8320,
  "path": "/axis-cgi/mjpg/video.cgi"
}
```

### 2. Start the Relay

```bash
cd /home/tri/orb/relay
cargo run
```

### 3. Open the Web App

```bash
cd /home/tri/orb/app
npm run dev
```

Navigate to `http://localhost:5173` and enter service ID: `axis-mjpeg`

### 4. Verify Logs

You should see:

```
[WebRtcBridge] MJPEG service detected, using transcoder
[WebRtcBridge] Starting MJPEG transcoder for http://83.48.75.113:8320/axis-cgi/mjpg/video.cgi
[MjpegTranscoder] Creating pipeline: souphttpsrc location=... ! multipartdemux ! jpegdec ! ...
[MjpegTranscoder] Starting pipeline
[MjpegTranscoder] State changed: Null -> Ready
[MjpegTranscoder] State changed: Ready -> Paused
[MjpegTranscoder] State changed: Paused -> Playing
[ingest_rtp] Received 1432 bytes, first byte: 0x80
[ingest_rtp] RTP: seq=12345, timestamp=90000, marker=false, payload_len=1420
[ingest_rtp] Forwarded RTP packet to WebRTC track
```

## GStreamer Pipeline Breakdown

### Element Flow

```
souphttpsrc
  ↓ (HTTP MJPEG stream - multipart/x-mixed-replace)
multipartdemux
  ↓ (Individual JPEG frames - NO TIMESTAMPS! ⚠️)
jpegdec
  ↓ (Raw video frames - still no timestamps)
clocksync ⭐ CRITICAL
  ↓ (Frames with pipeline clock timestamps)
videorate ⭐ CRITICAL
  ↓ (Frames at consistent 25fps framerate)
videoconvert
  ↓ (YUV420P/I420 for encoder)
x264enc (tune=zerolatency, speed-preset=ultrafast, bitrate=2000kbps)
  ↓ (H.264 NAL units)
h264parse (config-interval=-1)
  ↓ (H.264 byte-stream with SPS/PPS)
rtph264pay (pt=96)
  ↓ (RTP packets with H.264 payload)
appsink
  ↓ (Rust callback - sends to mpsc channel)
WebRTC Track
```

**Important**: MJPEG streams have **NO timing information**. The `clocksync` and `videorate` elements are absolutely critical:
- **`clocksync`** - Assigns timestamps based on the pipeline clock (when frames arrive)
- **`videorate`** - Enforces a consistent framerate (drops/duplicates frames to maintain 25fps)

### Key Configuration

- **`clocksync`** - Assigns timestamps to frames (MJPEG has no timing!)
- **`videorate`** - Enforces consistent framerate (25fps)
- **`framerate=25/1`** - Target 25 frames per second
- **`tune=zerolatency`** - Minimize encoder latency for real-time streaming
- **`speed-preset=ultrafast`** - Fastest encoding (trades compression for speed)
- **`bitrate=2000`** - 2 Mbps target (adjust based on your needs)
- **`key-int-max=50`** - Keyframe every 2 seconds at 25fps
- **`config-interval=-1`** - Send SPS/PPS in-band with IDR frames
- **`pt=96`** - RTP payload type 96 (H.264)
- **`max-buffers=1 drop=true`** - Drop old frames to minimize latency

## Performance Considerations

### Latency

- **GStreamer pipeline**: ~50-150ms
- **Network transmission**: ~20-50ms
- **WebRTC buffering**: ~50-100ms
- **Total glass-to-glass**: ~150-300ms

### CPU Usage

MJPEG transcoding is CPU-intensive:
- **640x480 @ 25fps**: ~15-25% CPU (single core)
- **1280x720 @ 30fps**: ~40-60% CPU (single core)
- **1920x1080 @ 30fps**: ~80-100% CPU (single core)

### Hardware Acceleration (Future)

To reduce CPU usage, you can use hardware encoders:

```rust
// Replace x264enc with hardware encoder:
// NVIDIA: "nvh264enc"
// Intel: "vaapih264enc"
// AMD: "vah264enc"

let pipeline_str = format!(
    "souphttpsrc location={} ! \
     multipartdemux ! \
     jpegdec ! \
     videoconvert ! \
     nvh264enc preset=low-latency ! \  // NVIDIA GPU
     h264parse ! \
     rtph264pay config-interval=1 pt=96 ! \
     appsink name=sink",
    url
);
```

## Comparison: RTSP vs MJPEG

| Feature | RTSP (Direct) | MJPEG (Transcoded) |
|---------|--------------|-------------------|
| Latency | 20-50ms | 150-300ms |
| CPU Usage | <5% | 15-100% (depends on resolution) |
| Bandwidth | Low (pre-compressed) | High (JPEG + H.264) |
| Quality | Original codec | Re-encoded (quality loss) |
| Codec Support | H.264, VP8, VP9 | Any (transcoded to H.264) |

## Troubleshooting

### Pipeline Fails to Start

**Error**: `Failed to create pipeline`

**Cause**: Missing GStreamer plugins

**Fix**: Install GStreamer plugins:
```bash
# Arch Linux
sudo pacman -S gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-plugins-ugly

# Ubuntu/Debian
sudo apt install gstreamer1.0-tools gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
                 gstreamer1.0-plugins-bad gstreamer1.0-libav gstreamer1.0-plugins-ugly
```

### High CPU Usage

**Issue**: Transcoding uses too much CPU

**Solutions**:
1. **Lower resolution** - Reduce camera resolution to 640x480
2. **Lower framerate** - Set camera to 15fps instead of 30fps
3. **Hardware encoding** - Use NVIDIA/Intel/AMD hardware encoder
4. **Increase preset** - Change `ultrafast` to `fast` (better compression, slightly higher latency)

### Stream Freezes / Buffering

**Issue**: Video stutters or freezes

**Solutions**:
1. **Check network** - Ensure stable connection to camera
2. **Increase buffer** - Modify GStreamer pipeline to add buffering
3. **Lower bitrate** - Reduce `bitrate=2000` to `bitrate=1000`

### No Video in Browser

**Issue**: WebRTC connects but no video appears

**Debug**:
1. Check relay logs for `[ingest_rtp]` messages - should see RTP packets
2. Check browser console for errors
3. Verify GStreamer pipeline is in `Playing` state
4. Test camera URL directly: `ffplay http://83.48.75.113:8320/axis-cgi/mjpg/video.cgi`

## Future Improvements

### 1. Codec Detection
Automatically detect codec from HTTP response headers instead of relying on `svc_type`.

### 2. Dynamic Bitrate
Adjust encoding bitrate based on network conditions (WebRTC bandwidth estimation).

### 3. Multiple Quality Profiles
Support simulcast or multiple encodings for adaptive streaming.

### 4. Audio Support
Currently video-only. Add audio transcoding for cameras with audio streams.

### 5. Hardware Acceleration
Auto-detect and use available hardware encoders (NVENC, QuickSync, VAAPI).

## Code Structure

### Files Modified

- **`Cargo.toml`** - Added GStreamer dependencies
- **`src/lib.rs`** - Added `mjpeg_transcoder` module
- **`src/mjpeg_transcoder.rs`** - New transcoder implementation
- **`src/webrtc.rs`** - Added MJPEG detection and transcoding logic

### Key Components

**MjpegTranscoder** (`src/mjpeg_transcoder.rs`):
- `new(url, rtp_tx)` - Create transcoder with HTTP URL
- `start()` - Start GStreamer pipeline
- `stop()` - Stop pipeline
- `wait_for_eos()` - Async wait for pipeline end/error

**WebRtcBridge** (`src/webrtc.rs`):
- `start_rtsp_client()` - Modified to detect and handle MJPEG services

## Dependencies

### Runtime Requirements

- **GStreamer 1.0+** - Core streaming framework
- **gst-plugins-base** - Basic elements (videoconvert, etc.)
- **gst-plugins-good** - HTTP source, JPEG decoder, RTP payloader
- **gst-plugins-ugly** - x264 encoder (GPL licensed)

### Rust Crates

- **gstreamer = "0.23"** - GStreamer Rust bindings
- **gstreamer-app = "0.23"** - AppSink/AppSrc support
- **gstreamer-video = "0.23"** - Video-specific utilities

## License Note

The x264 encoder (in `gst-plugins-ugly`) is **GPL-licensed**. If you're distributing binaries, be aware of GPL requirements. For a more permissive alternative, consider:
- **openh264** (BSD license, Cisco) - but lower quality
- **Hardware encoders** (depends on vendor)

---

**Ready to test!** Start the relay and point it at the Axis camera MJPEG stream.
