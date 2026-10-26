'use strict';

const net = require('net');

const DEFAULT_TIMEOUT_MS = 5000;

async function createControlledProxy(targetHost, targetPort, { timeoutMs = DEFAULT_TIMEOUT_MS } = {}) {
  const sockets = new Set();
  const downstreams = new Set();
  const queued = [];
  const waiters = [];
  const responseWaiters = [];
  let armed = false;
  let held = [];
  let closed = false;

  function notifyObserved() {
    while (waiters.length && queued.length) waiters.shift()(queued.shift());
  }

  const server = net.createServer((downstream) => {
    sockets.add(downstream);
    downstreams.add(downstream);
    const upstream = net.connect(targetPort, targetHost);
    sockets.add(upstream);
    downstream.on('data', (chunk) => {
      if (armed) {
        queued.push(chunk.length);
        notifyObserved();
      }
      if (!upstream.destroyed) upstream.write(chunk);
    });
    upstream.on('data', (chunk) => {
      if (armed) {
        held.push(Buffer.from(chunk));
        while (responseWaiters.length) responseWaiters.shift()();
      } else if (!downstream.destroyed) downstream.write(chunk);
    });
    for (const socket of [downstream, upstream]) {
      socket.on('error', () => {});
      socket.on('close', () => sockets.delete(socket));
    }
    downstream.on('close', () => upstream.destroy());
    upstream.on('close', () => downstream.destroy());
  });

  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const address = server.address();

  return {
    host: '127.0.0.1',
    port: address.port,
    arm() {
      armed = true;
    },
    rearm() {
      armed = true;
    },
    waitForClientData(label) {
      if (queued.length) return Promise.resolve(queued.shift());
      let timer;
      let resolveWaiter;
      const observed = new Promise((resolve) => {
        resolveWaiter = (value) => {
          clearTimeout(timer);
          resolve(value);
        };
        waiters.push(resolveWaiter);
      });
      const timeout = new Promise((_, reject) => {
        timer = setTimeout(() => {
          const index = waiters.indexOf(resolveWaiter);
          if (index >= 0) waiters.splice(index, 1);
          reject(new Error(`timed out waiting for ${label}`));
        }, timeoutMs);
      });
      return Promise.race([observed, timeout]).finally(() => clearTimeout(timer));
    },
    clearObservedClientData() {
      queued.length = 0;
    },
    waitForHeldResponse(label) {
      if (held.length) return Promise.resolve();
      let timer;
      let resolveWaiter;
      const observed = new Promise((resolve) => {
        resolveWaiter = () => {
          clearTimeout(timer);
          resolve();
        };
        responseWaiters.push(resolveWaiter);
      });
      const timeout = new Promise((_, reject) => {
        timer = setTimeout(() => {
          const index = responseWaiters.indexOf(resolveWaiter);
          if (index >= 0) responseWaiters.splice(index, 1);
          reject(new Error(`timed out waiting for ${label}`));
        }, timeoutMs);
      });
      return Promise.race([observed, timeout]).finally(() => clearTimeout(timer));
    },
    releaseResponses() {
      armed = false;
      queued.length = 0;
      for (const socket of downstreams) {
        if (!socket.destroyed) for (const chunk of held) socket.write(chunk);
      }
      held = [];
    },
    async close() {
      if (closed) return;
      closed = true;
      while (waiters.length) waiters.shift()(0);
      while (responseWaiters.length) responseWaiters.shift()();
      for (const socket of sockets) socket.destroy();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

module.exports = { createControlledProxy };
