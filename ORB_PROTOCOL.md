# ORB Protocol v1

## Overview

ORB v1 defines a minimal, TCP-based bridging protocol with strict separation of concerns:

- **Relay**: Public traffic router and multiplexer
- **Node**: Private TCP forwarder
- **Client**: Handles all protocol, path, and authentication logic

The Relay and Node never interpret application data.

- Paths (e.g., /stream1) live inside application protocols
- Authentication lives inside application protocols
- Relay and Node are unaware of both

## Client Connection Options

- Client connects via HTTPS (or WSS) to sni.relay_domain.com
- The relay can use SNI (Server Name Indication) to know which node or service to connect to.

# Client & Bridge

# Bridge Sharing (Not Yet Implemented)

A future enhancement could allow multiple clients to share a single bridge, reducing resource usage:

- **Subscriber tracking**: The bridge maintains a subscriber count
- **Lifecycle management**:
  - Bridge created when count goes from 0 → 1 (first client connects)
  - Bridge destroyed when count goes from 1 → 0 (last client disconnects)
- **Efficiency**: One TCP connection serves multiple clients for the same service

## Roles

### Relay

- Publicly reachable
- Manages Nodes and Clients
- Creates and multiplexes bridges

### Node

- Runs in private network
- Maintains one persistent TLS connection to Relay
- Opens TCP connections on demand
- Forwards raw bytes

### Client

- Connects to Relay
- Speaks application protocols (RTSP, HTTP, etc.)
- Handles paths, auth, and sessions

## Core Concepts

### Control Connection

The persistent TLS connection between Node and Relay that serves as the communication channel for:

- Control messages (bridge management, registration)
- Multiplexed data streams
- Keep-alive signals

### Bridge

A logical data channel that represents one TCP connection:

- Uniquely identified by `bridge_id`
- Maps: Client ↔ Relay ↔ Node ↔ Target Service
- Currently supports 1:1 client-to-bridge mapping

### Service

A target service that the Node can reach within its private network:

```json
{
  "addr": "192.168.1.100", // Private IP address
  "port": 554 // TCP port number
}
```

The Relay stores services but never interprets them - they're opaque identifiers for the Node.

## Message Encoding

All messages are CBOR-encoded maps with exactly one CBOR object per message.

## Messages

### REGISTER (Node → Relay)

```json
{
  "control": {
    "type": "register",
    "node_id": "node-001",
    "msg_id": "msg-001"
  }
}
```

### ANNOUNCE (Node → Relay)

Declares services reachable by the Node.

```json
{
  "control": {
    "type": "announce",
    "services": [
      {
        "addr": "192.168.1.100",
        "port": 554
      },
      {
        "addr": "127.0.0.1",
        "port": 443
      }
    ],
    "msg_id": "msg-002"
  }
}
```

Relay stores announced services but does not interpret them.

### ACK (Bidirectional)

```json
{
  "control": {
    "type": "ack",
    "msg_id": "optional"
  }
}
```

### OPEN_BRIDGE (Relay → Node)

```json
{
  "control": {
    "type": "open_bridge",
    "bridge_id": "bridge-123",
    "service": {
      "addr": "192.168.1.100",
      "port": 554
    },
    "msg_id": "msg-003"
  }
}
```

### CLOSE_BRIDGE (Relay → Node)

```json
{
  "control": {
    "type": "close_bridge",
    "bridge_id": "bridge-123",
    "msg_id": "msg-004"
  }
}
```

### DATA (Bidirectional)

```json
{
  "data": {
    "bridge_id": "bridge-123",
    "payload": <byte string>
  }
}
```

## Flows

### 1. Node Registration Flow

```
Node  → Relay : REGISTER
Relay → Node  : ACK
```

### 2. Service Announcement Flow (Repeatable)

```
Node → Relay : ANNOUNCE(services)
Relay → Node  : ACK
```

### 3. Bridge Creation Flow

```
Client → Relay : Connect
Relay → Node   : OPEN_BRIDGE(service, bridge_id)
Node  → Relay  : ACK
Node opens TCP connection to service.addr:service.port
```

### 4. Data Forwarding Flow

```
Client → Relay : application bytes
Relay → Node   : DATA(bridge_id, payload)
Node  → Target : payload

Target → Node  : payload
Node   → Relay : DATA(bridge_id, payload)
Relay  → Client: payload
```

Payload is never inspected or modified.

### 5. Bridge Teardown Flow

```
Client disconnects OR error
Relay → Node : CLOSE_BRIDGE(bridge_id)
Node closes local TCP socket
```
