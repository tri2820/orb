import { createSignal, onMount, onCleanup, type Component } from 'solid-js';

const WebRTCStream: Component = () => {
  const [video, setVideo] = createSignal<HTMLVideoElement>();
  const [status, setStatus] = createSignal('');
  const [serviceId, setServiceId] = createSignal('rtsp-camera');
  const [pc, setPc] = createSignal<RTCPeerConnection>();
  const [statsInterval, setStatsInterval] = createSignal<number>();

  // Use relative URL - Vite proxy will handle routing in development
  // In production, bow-server serves the app so /offer works directly

  onMount(() => {
    const videoEl = document.getElementById('video') as HTMLVideoElement;
    setVideo(videoEl);
  });

  onCleanup(() => {
    const interval = statsInterval();
    if (interval) {
      clearInterval(interval);
    }
    const peerConnection = pc();
    if (peerConnection) {
      peerConnection.close();
    }
  });

  const startSession = async () => {
    const videoEl = video();
    if (!videoEl) return;

    setStatus('Creating PeerConnection...');

    const newPc = new RTCPeerConnection({
      iceServers: [{
        urls: 'stun:stun.l.google.com:19302'
      }]
    });

    newPc.ontrack = (event) => {
      console.log('Got track:', event.streams);
      videoEl.srcObject = event.streams[0];
    };

    newPc.oniceconnectionstatechange = () => {
      console.log('ICE state:', newPc.iceConnectionState);
      setStatus('ICE state: ' + newPc.iceConnectionState);
    };

    // Add transceiver for video
    newPc.addTransceiver('video', { direction: 'recvonly' });

    const offer = await newPc.createOffer();
    await newPc.setLocalDescription(offer);

    setStatus('Sending offer...');

    try {
      const response = await fetch('/offer', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json'
        },
        body: JSON.stringify({
          sdp: newPc.localDescription?.sdp,
          serviceId: serviceId()
        })
      });

      const answer = await response.json();
      setStatus('Received answer. Setting remote description...');
      await newPc.setRemoteDescription(new RTCSessionDescription(answer));
      setPc(newPc);

      // Start logging stats
      const interval = setInterval(async () => {
        const stats = await newPc.getStats();
        stats.forEach(report => {
          if (report.type === 'inbound-rtp' && report.kind === 'video') {
            console.log(`[Stats] Jitter: ${report.jitter}, Packets: ${report.packetsReceived}, Frames Decoded: ${report.framesDecoded}, KeyFrames: ${report.keyFramesDecoded}`);
          }
        });
      }, 1000);
      setStatsInterval(interval as any);

    } catch (e: any) {
      console.error(e);
      setStatus('Error: ' + e.message);
    }
  };

  return (
    <div class="p-8">
      <h1 class="text-3xl font-bold mb-6">Bow WebRTC Stream</h1>

      <div class="mb-4">
        <video
          id="video"
          autoplay
          playsinline
          controls
          class="w-full max-w-4xl bg-black"
        />
      </div>

      <div class="flex gap-4 items-center mb-4">
        <input
          type="text"
          id="serviceId"
          value={serviceId()}
          onInput={(e) => setServiceId(e.currentTarget.value)}
          placeholder="Service ID"
          class="px-3 py-2 border rounded"
        />
        <button
          onClick={startSession}
          class="px-4 py-2 bg-blue-500 text-white rounded hover:bg-blue-600"
        >
          Start Stream
        </button>
      </div>

      <div id="status" class="text-lg">
        {status()}
      </div>
    </div>
  );
};

export default WebRTCStream;