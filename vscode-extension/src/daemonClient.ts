/**
 * Client for communicating with Cairn daemon via Unix socket
 */

import * as net from 'net';
import * as path from 'path';
import * as fs from 'fs';
import { spawn } from 'child_process';

export interface DaemonRequest {
    command: string;
    args: string[];
    cwd?: string;
}

export interface DaemonResponse {
    status: 'Success' | 'Error';
    message: string;
    data?: string;
}

export class DaemonClient {
    private socketPath: string;

    constructor() {
        const xdgRuntimeDir = process.env.XDG_RUNTIME_DIR || '/tmp';
        this.socketPath = path.join(xdgRuntimeDir, 'cairn-daemon.sock');
    }

    /**
     * Check if daemon is running
     */
    isDaemonRunning(): boolean {
        return fs.existsSync(this.socketPath);
    }

    /**
     * Start the daemon in the background
     */
    async startDaemon(): Promise<void> {
        if (this.isDaemonRunning()) {
            return;  // Already running
        }

        return new Promise((resolve, reject) => {
            const proc = spawn('cairn', ['daemon'], {
                detached: true,
                stdio: 'ignore'
            });

            proc.unref();  // Allow parent to exit independently

            // Wait for socket to appear
            let attempts = 0;
            const checkInterval = setInterval(() => {
                attempts++;
                if (this.isDaemonRunning()) {
                    clearInterval(checkInterval);
                    resolve();
                } else if (attempts > 20) {  // 2 seconds timeout
                    clearInterval(checkInterval);
                    reject(new Error('Daemon failed to start'));
                }
            }, 100);
        });
    }

    /**
     * Send a command to the daemon
     */
    async sendCommand(command: string, args: string[] = [], cwd?: string): Promise<DaemonResponse> {
        // Auto-start daemon if not running
        if (!this.isDaemonRunning()) {
            await this.startDaemon();
        }

        return new Promise((resolve, reject) => {
            const client = net.createConnection(this.socketPath, () => {
                const request: DaemonRequest = {
                    command,
                    args,
                    cwd
                };

                // Send request as JSON line
                client.write(JSON.stringify(request) + '\n');
            });

            let responseData = '';

            client.on('data', (data) => {
                responseData += data.toString();
            });

            client.on('end', () => {
                try {
                    const response: DaemonResponse = JSON.parse(responseData.trim());
                    resolve(response);
                } catch (e) {
                    reject(new Error(`Failed to parse daemon response: ${responseData}`));
                }
            });

            client.on('error', (err) => {
                reject(err);
            });
        });
    }

    /**
     * Ping the daemon
     */
    async ping(): Promise<boolean> {
        try {
            const response = await this.sendCommand('ping');
            return response.status === 'Success';
        } catch {
            return false;
        }
    }

    /**
     * Jump to a patch
     */
    async jump(patchHash: string, cwd: string): Promise<DaemonResponse> {
        return this.sendCommand('jump', [patchHash], cwd);
    }

    /**
     * Build project
     */
    async build(release: boolean, cwd: string): Promise<DaemonResponse> {
        const args = release ? ['--release'] : [];
        return this.sendCommand('build', args, cwd);
    }

    /**
     * Run project
     */
    async run(release: boolean, cwd: string): Promise<DaemonResponse> {
        const args = release ? ['--release'] : [];
        return this.sendCommand('run', args, cwd);
    }

    /**
     * Check project
     */
    async check(cwd: string): Promise<DaemonResponse> {
        return this.sendCommand('check', [], cwd);
    }

    /**
     * Test project
     */
    async test(cwd: string): Promise<DaemonResponse> {
        return this.sendCommand('test', [], cwd);
    }

    /**
     * Clear cairn directory
     */
    async clear(cwd: string): Promise<DaemonResponse> {
        return this.sendCommand('clear', [], cwd);
    }

    /**
     * List patches
     */
    async list(cwd: string): Promise<DaemonResponse> {
        return this.sendCommand('list', [], cwd);
    }
}
