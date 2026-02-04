/**
 * Pipe-based client for communicating with Cairn daemon via stdin/stdout
 */

import * as child_process from 'child_process';
import * as path from 'path';

// Command bytes (bit flags for clarity, though we send one at a time)
const CMD_PING = 0x01;   // 1
const CMD_JUMP = 0x02;   // 2
const CMD_BUILD = 0x04;  // 4
const CMD_CLEAR = 0x08;  // 8
const CMD_TEST = 0x10;   // 16
const CMD_CHECK = 0x20;  // 32
const CMD_RUN = 0x40;    // 64
const CMD_LIST = 0x80;   // 128

interface DaemonRequest {
    cwd?: string;
    args: string[];
}

interface DaemonResponse {
    status: 'Success' | 'Error';
    message: string;
    data?: string;
}

export class DaemonPipeClient {
    private daemon: child_process.ChildProcess | null = null;
    private responseBuffer: string = '';
    private pendingResponse: {
        resolve: (value: DaemonResponse) => void;
        reject: (reason: any) => void;
    } | null = null;

    constructor(private daemonPath: string) {}

    /**
     * Start the daemon process
     */
    start(): void {
        if (this.daemon) {
            return; // Already running
        }

        this.daemon = child_process.spawn(this.daemonPath, ['daemon'], {
            stdio: ['pipe', 'pipe', 'pipe'], // stdin, stdout, stderr
        });

        // Handle stdout (responses)
        this.daemon.stdout?.on('data', (data: Buffer) => {
            this.responseBuffer += data.toString('utf-8');
            this.processResponses();
        });

        // Handle stderr (logs)
        this.daemon.stderr?.on('data', (data: Buffer) => {
            const msg = data.toString('utf-8').trim();
            console.log('[DAEMON]', msg);
        });

        // Handle daemon exit
        this.daemon.on('exit', (code) => {
            console.log(`[DAEMON] Exited with code ${code}`);
            this.daemon = null;
            if (this.pendingResponse) {
                this.pendingResponse.reject(new Error('Daemon exited'));
                this.pendingResponse = null;
            }
        });
    }

    /**
     * Stop the daemon
     */
    stop(): void {
        if (this.daemon) {
            this.daemon.stdin?.end();
            this.daemon.kill();
            this.daemon = null;
        }
    }

    /**
     * Check if daemon is running
     */
    isRunning(): boolean {
        return this.daemon !== null && !this.daemon.killed;
    }

    /**
     * Process accumulated response data
     */
    private processResponses(): void {
        const lines = this.responseBuffer.split('\n');

        // Keep the last incomplete line in the buffer
        this.responseBuffer = lines.pop() || '';

        for (const line of lines) {
            if (!line.trim()) continue;

            try {
                const response: DaemonResponse = JSON.parse(line);
                if (this.pendingResponse) {
                    this.pendingResponse.resolve(response);
                    this.pendingResponse = null;
                }
            } catch (e) {
                console.error('[DAEMON] Failed to parse response:', line, e);
            }
        }
    }

    /**
     * Send command and wait for response
     */
    async sendCommand(cmdByte: number, args: string[] = [], cwd?: string): Promise<DaemonResponse> {
        if (!this.isRunning()) {
            this.start();
        }

        if (!this.daemon?.stdin) {
            throw new Error('Daemon stdin not available');
        }

        // Build request
        const request: DaemonRequest = {
            cwd,
            args
        };

        // Send command byte
        this.daemon.stdin.write(Buffer.from([cmdByte]));

        // Send request JSON (newline-terminated)
        this.daemon.stdin.write(JSON.stringify(request) + '\n');

        // Wait for response
        return new Promise((resolve, reject) => {
            this.pendingResponse = { resolve, reject };

            // Timeout after 30 seconds
            setTimeout(() => {
                if (this.pendingResponse) {
                    this.pendingResponse.reject(new Error('Daemon timeout'));
                    this.pendingResponse = null;
                }
            }, 30000);
        });
    }

    /**
     * Send PING command
     */
    async ping(): Promise<DaemonResponse> {
        return this.sendCommand(CMD_PING, []);
    }

    /**
     * Send CLEAR command
     */
    async clear(cwd: string): Promise<DaemonResponse> {
        return this.sendCommand(CMD_CLEAR, [], cwd);
    }

    /**
     * Send JUMP command
     */
    async jump(patchHash: string, cwd: string): Promise<DaemonResponse> {
        return this.sendCommand(CMD_JUMP, [patchHash], cwd);
    }

    /**
     * Send BUILD command
     */
    async build(args: string[], cwd: string): Promise<DaemonResponse> {
        return this.sendCommand(CMD_BUILD, args, cwd);
    }

    /**
     * Send TEST command
     */
    async test(cwd: string): Promise<DaemonResponse> {
        return this.sendCommand(CMD_TEST, [], cwd);
    }

    /**
     * Send CHECK command
     */
    async check(cwd: string): Promise<DaemonResponse> {
        return this.sendCommand(CMD_CHECK, [], cwd);
    }

    /**
     * Send RUN command
     */
    async run(args: string[], cwd: string): Promise<DaemonResponse> {
        return this.sendCommand(CMD_RUN, args, cwd);
    }

    /**
     * Send LIST command
     */
    async list(cwd: string): Promise<DaemonResponse> {
        return this.sendCommand(CMD_LIST, [], cwd);
    }
}
