// Runs ArduPilot SITL (WebAssembly) and relays its serial ports over a WebSocket to the app.
// Frames are [serial number][bytes] in both directions. SERIAL0, 1 and 2 use the wasm backend.
// A frame that starts with 0xFF carries the firmware's eeprom.bin, so the app keeps the parameters.
const BUFFER_SIZE = 16384;
const SERIALS = [0, 1, 2];
const POLL_MS = 5;
const EEPROM_FRAME = 0xff;
const EEPROM_MS = 1000;

const params = new URL(self.location.href).searchParams;
const binary = params.get('binary');
const args = params.getAll('arg');
const ws = new WebSocket(`ws://${self.location.host}/sitl/ws`);
ws.binaryType = 'arraybuffer';

const opened = new Promise((resolve, reject) => {
  ws.onopen = resolve;
  ws.onerror = () => reject(new Error('no WebSocket to the app'));
});

function fail(message) {
  self.postMessage('SITL stopped: ' + message);
  if (ws.readyState === WebSocket.OPEN) {
    ws.send('error: ' + message);
    ws.close();
  }
}

async function main() {
  await opened;
  if (!self.crossOriginIsolated || typeof SharedArrayBuffer === 'undefined') {
    fail('this WebView gives no SharedArrayBuffer (cross-origin isolation is off)');
    return;
  }
  if (!/^ardu(copter|plane)$/.test(binary ?? '')) {
    fail('unknown firmware ' + binary);
    return;
  }
  const saved = await fetch('eeprom', { cache: 'no-store' })
    .then((r) => (r.ok ? r.arrayBuffer() : null))
    .catch(() => null);
  let eepromPath = 'eeprom.bin';
  const { default: createModule } = await import(`./${binary}.js`);
  const sitl = await createModule({
    arguments: [...args, '--serial0', 'wasm', '--serial1', 'wasm', '--serial2', 'wasm'],
    print: (line) => console.log(line),
    printErr: (line) => console.warn(line),
    // SITL opens eeprom.bin in its working directory. Put the saved one there before main() runs.
    preRun: [
      (module) => {
        eepromPath = module.FS.cwd().replace(/\/$/, '') + '/eeprom.bin';
        if (saved && saved.byteLength > 0) module.FS.writeFile(eepromPath, new Uint8Array(saved));
      },
    ],
  });
  ws.send(`started ${binary} ${args.join(' ')} (saved parameters: ${saved ? saved.byteLength + ' bytes' : 'none'})`);

  const malloc = sitl.cwrap('ardupilot_malloc', 'number', ['number']);
  const read = sitl.cwrap('ardupilot_serial_read', 'number', ['number', 'number', 'number']);
  const write = sitl.cwrap('ardupilot_serial_write', 'number', ['number', 'number', 'number']);
  const readBuffer = malloc(BUFFER_SIZE);
  const writeBuffer = malloc(BUFFER_SIZE);

  ws.onmessage = ({ data }) => {
    if (!(data instanceof ArrayBuffer) || data.byteLength < 2) return;
    const frame = new Uint8Array(data);
    const length = Math.min(frame.length - 1, BUFFER_SIZE);
    sitl.HEAPU8.set(frame.subarray(1, 1 + length), writeBuffer);
    write(frame[0], writeBuffer, length);
  };
  ws.onclose = () => {
    self.postMessage('SITL stopped. You can close this page.');
    self.close();
  };

  self.postMessage('ArduPilot SITL running: ' + binary);
  setInterval(() => {
    if (ws.readyState !== WebSocket.OPEN) return;
    for (const serial of SERIALS) {
      const length = read(serial, readBuffer, BUFFER_SIZE - 1);
      if (length > 0) {
        const frame = new Uint8Array(length + 1);
        frame[0] = serial;
        frame.set(sitl.HEAPU8.subarray(readBuffer, readBuffer + length), 1);
        ws.send(frame);
      }
    }
  }, POLL_MS);

  let lastEeprom = saved ? new Uint8Array(saved) : null;
  setInterval(() => {
    if (ws.readyState !== WebSocket.OPEN) return;
    let now;
    try {
      now = sitl.FS.readFile(eepromPath);
    } catch {
      return;
    }
    if (lastEeprom && now.length === lastEeprom.length && now.every((b, i) => b === lastEeprom[i])) return;
    lastEeprom = now;
    const frame = new Uint8Array(now.length + 1);
    frame[0] = EEPROM_FRAME;
    frame.set(now, 1);
    ws.send(frame);
  }, EEPROM_MS);
}

main().catch((err) => fail(String(err?.message ?? err)));
