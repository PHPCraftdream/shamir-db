# shamir-transport-tcp — оптимизация производительности

## Обзор
Length-prefix framing поверх TLS/TCP. Чтение/запись frame.

## Вывод
Простой framing layer. I/O-bound, не CPU-bound.

## 🟢 Мелкие
- Buffer reuse: `read_frame_into` может использовать pre-allocated buffer вместо свежего Vec на каждый frame.
- Zero-copy: для больших frames можно использовать `Bytes::from(Vec)` вместо копирования.
