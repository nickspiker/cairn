import * as vscode from 'vscode';
import * as fs from 'fs';
import * as path from 'path';
import * as https from 'https';
import * as crypto from 'crypto';
import { promisify } from 'util';
import { exec } from 'child_process';

const execAsync = promisify(exec);

interface PlatformInfo {
    platform: string;
    arch: string;
    binaryName: string;
    downloadName: string;
}

export class BinaryManager {
    private context: vscode.ExtensionContext;
    private binaryPath: string | null = null;

    constructor(context: vscode.ExtensionContext) {
        this.context = context;
    }

    /**
     * Get the cairn binary path, downloading if necessary
     */
    async getCairnPath(): Promise<string> {
        // Check if we already have it cached
        if (this.binaryPath && fs.existsSync(this.binaryPath)) {
            return this.binaryPath;
        }

        // 1. Check if cairn is in PATH
        const pathBinary = await this.checkPath();
        if (pathBinary) {
            this.binaryPath = pathBinary;
            return pathBinary;
        }

        // 2. Check workspace local build (development)
        const localBinary = await this.checkLocalBuild();
        if (localBinary) {
            this.binaryPath = localBinary;
            return localBinary;
        }

        // 3. Check extension storage cache
        const cachedBinary = await this.checkCache();
        if (cachedBinary) {
            this.binaryPath = cachedBinary;
            return cachedBinary;
        }

        // 4. Download from GitHub releases
        const downloadedBinary = await this.downloadBinary();
        this.binaryPath = downloadedBinary;
        return downloadedBinary;
    }

    /**
     * Check if cairn is available in PATH
     */
    private async checkPath(): Promise<string | null> {
        try {
            const { stdout } = await execAsync(process.platform === 'win32' ? 'where cairn' : 'which cairn');
            const binaryPath = stdout.trim().split('\n')[0];
            if (fs.existsSync(binaryPath)) {
                return binaryPath;
            }
        } catch {
            // Not in PATH
        }
        return null;
    }

    /**
     * Check for local development build
     */
    private async checkLocalBuild(): Promise<string | null> {
        const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
        if (!workspaceFolder) {
            return null;
        }

        const platformInfo = this.getPlatformInfo();
        const candidates = [
            path.join(workspaceFolder.uri.fsPath, 'target', 'release', platformInfo.binaryName),
            path.join(workspaceFolder.uri.fsPath, 'target', 'debug', platformInfo.binaryName),
        ];

        for (const candidate of candidates) {
            if (fs.existsSync(candidate)) {
                return candidate;
            }
        }

        return null;
    }

    /**
     * Check extension storage cache
     */
    private async checkCache(): Promise<string | null> {
        const platformInfo = this.getPlatformInfo();
        const cachedPath = path.join(
            this.context.globalStorageUri.fsPath,
            'bin',
            platformInfo.binaryName
        );

        if (fs.existsSync(cachedPath)) {
            // Make sure it's executable
            try {
                fs.chmodSync(cachedPath, 0o755);
                return cachedPath;
            } catch {
                // If chmod fails, we'll re-download
            }
        }

        return null;
    }

    /**
     * Download cairn binary from GitHub releases
     */
    private async downloadBinary(): Promise<string> {
        const platformInfo = this.getPlatformInfo();
        const version = 'v0.0.0'; // TODO: Make this dynamic from package.json
        const baseUrl = `https://github.com/nickspiker/cairn/releases/download/${version}`;

        const binaryUrl = `${baseUrl}/cairn-${platformInfo.platform}-${platformInfo.arch}${process.platform === 'win32' ? '.exe' : ''}`;
        const hashUrl = `${binaryUrl}.b3`;

        return await vscode.window.withProgress(
            {
                location: vscode.ProgressLocation.Notification,
                title: 'Cairn',
                cancellable: false
            },
            async (progress) => {
                progress.report({ message: 'Downloading cairn binary...' });

                // Download hash first
                const expectedHash = await this.downloadText(hashUrl);

                // Download binary
                const binaryDir = path.join(this.context.globalStorageUri.fsPath, 'bin');
                if (!fs.existsSync(binaryDir)) {
                    fs.mkdirSync(binaryDir, { recursive: true });
                }

                const binaryPath = path.join(binaryDir, platformInfo.binaryName);
                await this.downloadFile(binaryUrl, binaryPath, (percent) => {
                    progress.report({ message: `Downloading cairn binary... ${percent}%` });
                });

                progress.report({ message: 'Verifying binary...' });

                // Verify BLAKE3 hash
                const actualHash = await this.blake3Hash(binaryPath);
                if (actualHash !== expectedHash.trim()) {
                    fs.unlinkSync(binaryPath);
                    throw new Error(`Binary verification failed. Expected ${expectedHash.trim()}, got ${actualHash}`);
                }

                // Make executable
                fs.chmodSync(binaryPath, 0o755);

                progress.report({ message: 'Cairn binary ready!' });

                return binaryPath;
            }
        );
    }

    /**
     * Get platform-specific information
     */
    private getPlatformInfo(): PlatformInfo {
        const platform = process.platform;
        const arch = process.arch;

        let platformName: string;
        let archName: string;
        let binaryName: string;

        if (platform === 'win32') {
            platformName = 'windows';
            archName = 'x64';
            binaryName = 'cairn.exe';
        } else if (platform === 'darwin') {
            platformName = 'darwin';
            archName = arch === 'arm64' ? 'arm64' : 'x64';
            binaryName = 'cairn';
        } else {
            platformName = 'linux';
            archName = 'x64';
            binaryName = 'cairn';
        }

        return {
            platform: platformName,
            arch: archName,
            binaryName,
            downloadName: `cairn-${platformName}-${archName}${platform === 'win32' ? '.exe' : ''}`
        };
    }

    /**
     * Download a file with progress
     */
    private downloadFile(url: string, dest: string, onProgress: (percent: number) => void): Promise<void> {
        return new Promise((resolve, reject) => {
            const file = fs.createWriteStream(dest);

            https.get(url, (response) => {
                if (response.statusCode === 302 || response.statusCode === 301) {
                    // Handle redirect
                    const redirectUrl = response.headers.location;
                    if (redirectUrl) {
                        https.get(redirectUrl, (redirectResponse) => {
                            const totalBytes = parseInt(redirectResponse.headers['content-length'] || '0', 10);
                            let downloadedBytes = 0;

                            redirectResponse.on('data', (chunk) => {
                                downloadedBytes += chunk.length;
                                if (totalBytes > 0) {
                                    const percent = Math.floor((downloadedBytes / totalBytes) * 100);
                                    onProgress(percent);
                                }
                            });

                            redirectResponse.pipe(file);
                            file.on('finish', () => {
                                file.close();
                                resolve();
                            });
                        }).on('error', reject);
                    }
                } else if (response.statusCode === 200) {
                    const totalBytes = parseInt(response.headers['content-length'] || '0', 10);
                    let downloadedBytes = 0;

                    response.on('data', (chunk) => {
                        downloadedBytes += chunk.length;
                        if (totalBytes > 0) {
                            const percent = Math.floor((downloadedBytes / totalBytes) * 100);
                            onProgress(percent);
                        }
                    });

                    response.pipe(file);
                    file.on('finish', () => {
                        file.close();
                        resolve();
                    });
                } else {
                    reject(new Error(`Download failed: HTTP ${response.statusCode}`));
                }
            }).on('error', (err) => {
                fs.unlinkSync(dest);
                reject(err);
            });

            file.on('error', (err) => {
                fs.unlinkSync(dest);
                reject(err);
            });
        });
    }

    /**
     * Download text file
     */
    private downloadText(url: string): Promise<string> {
        return new Promise((resolve, reject) => {
            https.get(url, (response) => {
                if (response.statusCode === 302 || response.statusCode === 301) {
                    const redirectUrl = response.headers.location;
                    if (redirectUrl) {
                        https.get(redirectUrl, (redirectResponse) => {
                            let data = '';
                            redirectResponse.on('data', chunk => data += chunk);
                            redirectResponse.on('end', () => resolve(data));
                        }).on('error', reject);
                    }
                } else if (response.statusCode === 200) {
                    let data = '';
                    response.on('data', chunk => data += chunk);
                    response.on('end', () => resolve(data));
                } else {
                    reject(new Error(`Download failed: HTTP ${response.statusCode}`));
                }
            }).on('error', reject);
        });
    }

    /**
     * Compute BLAKE3 hash of a file
     */
    private async blake3Hash(filePath: string): Promise<string> {
        // Try using b3sum if available
        try {
            const { stdout } = await execAsync(`b3sum "${filePath}"`);
            return stdout.trim().split(' ')[0];
        } catch {
            // Fall back to SHA256 if b3sum not available
            // TODO: Consider bundling a BLAKE3 JS implementation
            return this.sha256Hash(filePath);
        }
    }

    /**
     * Compute SHA256 hash as fallback
     */
    private sha256Hash(filePath: string): string {
        const fileBuffer = fs.readFileSync(filePath);
        const hashSum = crypto.createHash('sha256');
        hashSum.update(fileBuffer);
        return hashSum.digest('hex');
    }
}
