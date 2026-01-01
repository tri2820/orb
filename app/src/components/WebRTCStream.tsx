import { createSignal, onMount, onCleanup, For, type Component } from 'solid-js';

interface ServiceInfo {
  id: string;
  type: string;
}

const WebRTCStream: Component = () => {
  const [video, setVideo] = createSignal<HTMLVideoElement>();
  const [status, setStatus] = createSignal('');
  const [serviceId, setServiceId] = createSignal('');
  const [services, setServices] = createSignal<ServiceInfo[]>([]);
  const [pc, setPc] = createSignal<RTCPeerConnection>();
  const [statsInterval, setStatsInterval] = createSignal<number>();

  onMount(async () => {
    const videoEl = document.getElementById('video') as HTMLVideoElement;
    setVideo(videoEl);

    // Fetch available services
    try {
      const response = await fetch('/relay/services');
      const data = await response.json();
      setServices(data);
      if (data.length > 0) {
        setServiceId(data[0].id);
      }
    } catch (e) {
      console.error('Failed to fetch services:', e);
    }
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

      // Debug: capture a frame after 2 seconds
      setTimeout(() => {
        const canvas = document.createElement('canvas');
        canvas.width = videoEl.videoWidth || 640;
        canvas.height = videoEl.videoHeight || 480;
        const ctx = canvas.getContext('2d');
        if (ctx) {
          ctx.drawImage(videoEl, 0, 0);
          const imageData = ctx.getImageData(0, 0, Math.min(10, canvas.width), Math.min(10, canvas.height));
          console.log('[Debug] First 10x10 pixels:', Array.from(imageData.data.slice(0, 40)));
          console.log('[Debug] Canvas data URL (check if gray):', canvas.toDataURL().substring(0, 100));
        }
      }, 2000);
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
      const response = await fetch('/relay/offer', {
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
            console.log(`[Stats] At: ${new Date().toISOString()}, Jitter: ${report.jitter}, Packets: ${report.packetsReceived}, Frames Decoded: ${report.framesDecoded}, KeyFrames: ${report.keyFramesDecoded}`);
          }
        });
        // Debug video element state
        const v = video();
        if (v) {
          console.log(`[Video] paused=${v.paused}, readyState=${v.readyState}, videoWidth=${v.videoWidth}x${v.videoHeight}, currentTime=${v.currentTime.toFixed(2)}`);
        }
      }, 1000);
      setStatsInterval(interval as any);

    } catch (e: any) {
      console.error(e);
      setStatus('Error: ' + e.message);
    }
  };

  return (
    <div class="p-8">
      <h1 class="text-3xl font-bold mb-6">ORB WebRTC Stream</h1>

      <div class="mb-4">
        <video
          id="video"
          autoplay
          muted
          playsinline
          controls
          class="w-full max-w-4xl bg-black"
        />
      </div>

      <div class="flex gap-4 items-center mb-4">
        <select
          value={serviceId()}
          onChange={(e) => setServiceId(e.currentTarget.value)}
          class="px-3 py-2 border rounded min-w-48"
        >
          <For each={services()}>
            {(service) => (
              <option value={service.id}>
                {service.id} ({service.type})
              </option>
            )}
          </For>
        </select>
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

      <div class="mt-4 text-sm text-gray-500">
        Available services: {services().length}
      </div>
    </div>
  );
};

export default WebRTCStream;
