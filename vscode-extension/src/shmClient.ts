/**
 * Shared memory client for communicating with Cairn daemon
 * Uses POSIX shared memory with futex-based synchronization
 */

import * as fs from 'fs';
import * as path from 'path';

// Shared memory layout constants
const SHM_SIZE = 2060;
const TX_BUFFER_OFFSET = 12;
const TX_BUFFER_SIZE = 1024;
const RX_BUFFER_OFFSET = 12 + TX_BUFFER_SIZE;
const RX_BUFFER_SIZE = 1024;

// Event flags
const EVENT_PATCHES_CHANGED = 0x01;
const EVENT_BUILD_STARTED = 0x02;
const EVENT_BUILD_COMPLETED = 0x04;

export interface DaemonResponse {
    status: 'Success' | 'Error';
    message: string;
    data?: string;
}

export class DaemonShmClient {
    private shmPath: string;
    private fd: number | null = null;
    private buffer: SharedArrayBuffer | null = null;
    private view: DataView | null = null;
    private txReady: Int32Array | null = null;
    private rxReady: Int32Array | null = null;
    private events: Int32Array | null = null;

    constructor() {
        // Path to shared memory on Linux
        this.shmPath = '/dev/shm/cairn-daemon-shm';
    }

    /**
     * Check if daemon is running
     */
    isDaemonRunning(): boolean {
        try {
            return fs.existsSync(this.shmPath);
        } catch {
            return false;
        }
    }

    /**
     * Connect to daemon's shared memory
     */
    connect(): void {
        if (this.buffer) {
            return; // Already connected
        }

        if (!this.isDaemonRunning()) {
            throw new Error('Daemon is not running. Start it with: cairn daemon');
        }

        // Open shared memory file
        this.fd = fs.openSync(this.shmPath, 'r+');

        // Read the entire shared memory into a buffer
        const buffer = Buffer.alloc(SHM_SIZE);
        fs.readSync(this.fd, buffer, 0, SHM_SIZE, 0);

        // Create SharedArrayBuffer (for Atomics operations)
        this.buffer = new SharedArrayBuffer(SHM_SIZE);
        const uint8View = new Uint8Array(this.buffer);
        uint8View.set(buffer);

        this.view = new DataView(this.buffer);

        // Create typed arrays for atomic operations
        this.txReady = new Int32Array(this.buffer, 0, 1);
        this.rxReady = new Int32Array(this.buffer, 4, 1);
        this.events = new Int32Array(this.buffer, 8, 1);
    }

    /**
     * Disconnect from shared memory
     */
    disconnect(): void {
        if (this.fd !== null) {
            fs.closeSync(this.fd);
            this.fd = null;
        }
        this.buffer = null;
        this.view = null;
        this.txReady = null;
        this.rxReady = null;
        this.events = null;
    }

    /**
     * Write buffer to shared memory file
     */
    private syncToFile(): void {
        if (this.fd === null || !this.buffer) {
            throw new Error('Not connected to shared memory');
        }

        const uint8View = new Uint8Array(this.buffer);
        const buffer = Buffer.from(uint8View);
        fs.writeSync(this.fd, buffer, 0, SHM_SIZE, 0);
    }

    /**
     * Read buffer from shared memory file
     */
    private syncFromFile(): void {
        if (this.fd === null || !this.buffer) {
            throw new Error('Not connected to shared memory');
        }

        const buffer = Buffer.alloc(SHM_SIZE);
        fs.readSync(this.fd, buffer, 0, SHM_SIZE, 0);

        const uint8View = new Uint8Array(this.buffer);
        uint8View.set(buffer);
    }

    /**
     * Send command and wait for response
     */
    async sendCommand(command: string, args: string[] = [], cwd?: string): Promise<DaemonResponse> {
        if (!this.buffer || !this.txReady || !this.rxReady) {
            this.connect();
        }

        // Encode request as simple ASCII format
        // Format: "command arg1 arg2\n/path/to/cwd"
        let request = command;
        if (args.length > 0) {
            request += ' ' + args.join(' ');
        }
        if (cwd) {
            request += '\n' + cwd;
        }

        const requestBytes = Buffer.from(request, 'ascii');
        if (requestBytes.length > TX_BUFFER_SIZE) {
            throw new Error('Request too large');
        }

        // Write to tx_buffer
        const txBuffer = new Uint8Array(this.buffer!, TX_BUFFER_OFFSET, TX_BUFFER_SIZE);
        txBuffer.fill(0); // Clear buffer
        txBuffer.set(requestBytes);

        // Sync to file and set tx_ready flag
        this.syncToFile();
        Atomics.store(this.txReady!, 0, 1);
        this.syncToFile(); // Sync the flag change

        // Wait for rx_ready using busy-wait (Atomics.wait doesn't work with file-backed memory)
        const startTime = Date.now();
        const timeout = 30000; // 30 seconds

        while (true) {
            this.syncFromFile(); // Read latest state
            const rxReadyValue = Atomics.load(this.rxReady!, 0);

            if (rxReadyValue === 1) {
                break; // Response ready
            }

            if (Date.now() - startTime > timeout) {
                throw new Error('Daemon timeout - no response after 30s');
            }

            // Sleep briefly to avoid busy-waiting too hard
            await new Promise(resolve => setTimeout(resolve, 10));
        }

        // Read response from rx_buffer
        this.syncFromFile();
        const rxBuffer = new Uint8Array(this.buffer!, RX_BUFFER_OFFSET, RX_BUFFER_SIZE);

        // Find null terminator or end of buffer
        let responseLength = 0;
        for (let i = 0; i < RX_BUFFER_SIZE; i++) {
            if (rxBuffer[i] === 0) {
                responseLength = i;
                break;
            }
        }
        if (responseLength === 0) {
            responseLength = RX_BUFFER_SIZE;
        }

        const responseBytes = Buffer.from(rxBuffer.slice(0, responseLength));
        const responseStr = responseBytes.toString('ascii');

        // Parse response: "status\nmessage\ndata"
        const lines = responseStr.split('\n');
        const status = lines[0] as 'Success' | 'Error';
        const message = lines[1] || '';
        const data = lines.length > 2 ? lines.slice(2).join('\n') : undefined;

        // Clear rx_ready
        Atomics.store(this.rxReady!, 0, 0);
        this.syncToFile();

        return { status, message, data };
    }

    /**
     * Check for events (non-blocking)
     */
    pollEvents(): number {
        if (!this.events) {
            return 0;
        }

        this.syncFromFile();
        const eventFlags = Atomics.load(this.events, 0);

        // Clear events after reading
        if (eventFlags !== 0) {
            Atomics.store(this.events, 0, 0);
            this.syncToFile();
        }

        return eventFlags;
    }

    /**
     * Check if patches changed event is set
     */
    hasPatchesChanged(eventFlags: number): boolean {
        return (eventFlags & EVENT_PATCHES_CHANGED) !== 0;
    }

    /**
     * Check if build started event is set
     */
    hasBuildStarted(eventFlags: number): boolean {
        return (eventFlags & EVENT_BUILD_STARTED) !== 0;
    }

    /**
     * Check if build completed event is set
     */
    hasBuildCompleted(eventFlags: number): boolean {
        return (eventFlags & EVENT_BUILD_COMPLETED) !== 0;
    }

    // Convenience methods for commands

    async ping(): Promise<boolean> {
        try {
            const response = await this.sendCommand('ping');
            return response.status === 'Success';
        } catch {
            return false;
        }
    }

    async jump(patchHash: string, cwd: string): Promise<DaemonResponse> {
        return this.sendCommand('jump', [patchHash], cwd);
    }

    async build(release: boolean, cwd: string): Promise<DaemonResponse> {
        const args = release ? ['--release'] : [];
        return this.sendCommand('build', args, cwd);
    }

    async run(release: boolean, cwd: string): Promise<DaemonResponse> {
        const args = release ? ['--release'] : [];
        return this.sendCommand('run', args, cwd);
    }

    async check(cwd: string): Promise<DaemonResponse> {
        return this.sendCommand('check', [], cwd);
    }

    async test(cwd: string): Promise<DaemonResponse> {
        return this.sendCommand('test', [], cwd);
    }

    async clear(cwd: string): Promise<DaemonResponse> {
        return this.sendCommand('clear', [], cwd);
    }

    async list(cwd: string): Promise<DaemonResponse> {
        return this.sendCommand('list', [], cwd);
    }
}
