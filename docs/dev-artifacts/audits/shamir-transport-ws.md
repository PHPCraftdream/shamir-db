# shamir-transport-ws — оптимизация производительности

## Обзор
WebSocket framing поверх TLS/TCP.

## Вывод
Аналогично TCP transport. I/O-bound.

## 🟢 Мелкие
- `ws_recv_into_stream` / `ws_send_sink` — обёртки над tungstenite. Минимальный overhead.
- `MAX_WS_FRAME_SIZE` — configurable. ✅
