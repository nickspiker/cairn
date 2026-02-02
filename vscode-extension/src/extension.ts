import * as vscode from 'vscode';
import * as fs from 'fs';
import * as path from 'path';
import { DaemonShmClient } from './shmClient';

let outputChannel: vscode.OutputChannel;
let shmClient: DaemonShmClient;

class CairnTreeItem extends vscode.TreeItem {
    constructor(
        public readonly label: string,
        public readonly collapsibleState: vscode.TreeItemCollapsibleState,
        public readonly command?: vscode.Command,
        public readonly contextValue?: string
    ) {
        super(label, collapsibleState);
    }
}

class CairnTreeProvider implements vscode.TreeDataProvider<CairnTreeItem> {
    private _onDidChangeTreeData: vscode.EventEmitter<CairnTreeItem | undefined | null | void> = new vscode.EventEmitter<CairnTreeItem | undefined | null | void>();
    readonly onDidChangeTreeData: vscode.Event<CairnTreeItem | undefined | null | void> = this._onDidChangeTreeData.event;
    private refreshTimeout: NodeJS.Timeout | null = null;

    refresh(): void {
        // Debounce refreshes to prevent freezing
        if (this.refreshTimeout) {
            clearTimeout(this.refreshTimeout);
        }
        this.refreshTimeout = setTimeout(() => {
            outputChannel.appendLine('[REFRESH] Tree view refresh triggered');
            this._onDidChangeTreeData.fire();
            this.refreshTimeout = null;
        }, 300); // 300ms debounce
    }

    getTreeItem(element: CairnTreeItem): vscode.TreeItem {
        return element;
    }

    getChildren(element?: CairnTreeItem): CairnTreeItem[] {
        outputChannel.appendLine(`[TREE] getChildren called for: ${element?.label || 'root'}`);
        if (!element) {
            // Root level - show Commands and Patches sections
            return [
                new CairnTreeItem('Commands', vscode.TreeItemCollapsibleState.Expanded, undefined, 'commands'),
                new CairnTreeItem('Patches', vscode.TreeItemCollapsibleState.Expanded, undefined, 'patches'),
            ];
        } else if (element.contextValue === 'commands') {
            // Build commands
            return [
                new CairnTreeItem('▶ Run', vscode.TreeItemCollapsibleState.None, {
                    command: 'cairn.run',
                    title: 'Run',
                }),
                new CairnTreeItem('▶ Run (Release)', vscode.TreeItemCollapsibleState.None, {
                    command: 'cairn.runRelease',
                    title: 'Run Release',
                }),
                new CairnTreeItem('🔨 Build', vscode.TreeItemCollapsibleState.None, {
                    command: 'cairn.build',
                    title: 'Build',
                }),
                new CairnTreeItem('🔨 Build (Release)', vscode.TreeItemCollapsibleState.None, {
                    command: 'cairn.buildRelease',
                    title: 'Build Release',
                }),
                new CairnTreeItem('✓ Check', vscode.TreeItemCollapsibleState.None, {
                    command: 'cairn.check',
                    title: 'Check',
                }),
                new CairnTreeItem('🧪 Test', vscode.TreeItemCollapsibleState.None, {
                    command: 'cairn.test',
                    title: 'Test',
                }),
                new CairnTreeItem('🗑 Clear', vscode.TreeItemCollapsibleState.None, {
                    command: 'cairn.clear',
                    title: 'Clear',
                }),
            ];
        } else if (element.contextValue === 'patches') {
            // List patches from .cairn/patches directory
            outputChannel.appendLine('[PATCHES] Loading patches list');
            return this.getPatches();
        }
        return [];
    }

    private getCurrentPatchHash(workspaceRoot: string): string | null {
        try {
            // Simple heuristic: newest patch by mtime is current
            // This avoids blocking execSync calls that cause freezing
            const patchesDir = path.join(workspaceRoot, '.cairn', 'patches');
            if (!fs.existsSync(patchesDir)) {
                return null;
            }

            const patchFiles = fs.readdirSync(patchesDir, { withFileTypes: true })
                .filter(dirent => dirent.isFile() && dirent.name.endsWith('.vsf'))
                .map(dirent => {
                    const filePath = path.join(patchesDir, dirent.name);
                    const stats = fs.statSync(filePath);
                    return { hash: dirent.name.replace('.vsf', ''), mtime: stats.mtime };
                })
                .sort((a, b) => b.mtime.getTime() - a.mtime.getTime());

            if (patchFiles.length > 0) {
                const currentHash = patchFiles[0].hash;
                outputChannel.appendLine(`[STATE] Current patch (newest by mtime): ${currentHash.substring(0, 8)}`);
                return currentHash;
            }

            return null;
        } catch (err) {
            outputChannel.appendLine(`[STATE] Error getting current patch: ${err}`);
            return null;
        }
    }

    private getPatches(): CairnTreeItem[] {
        const workspaceFolders = vscode.workspace.workspaceFolders;
        if (!workspaceFolders) {
            outputChannel.appendLine('[PATCHES] No workspace folders');
            return [];
        }

        const workspaceRoot = workspaceFolders[0].uri.fsPath;
        const patchesDir = path.join(workspaceRoot, '.cairn', 'patches');
        outputChannel.appendLine(`[PATCHES] Reading from: ${patchesDir}`);

        if (!fs.existsSync(patchesDir)) {
            outputChannel.appendLine('[PATCHES] Directory does not exist');
            return [new CairnTreeItem('No patches yet', vscode.TreeItemCollapsibleState.None)];
        }

        // Get current patch hash
        const currentPatchHash = this.getCurrentPatchHash(workspaceRoot);

        try {
            const files = fs.readdirSync(patchesDir, { withFileTypes: true });
            outputChannel.appendLine(`[PATCHES] Found ${files.length} files`);

            const patches = files
                .filter(dirent => dirent.isFile() && dirent.name.endsWith('.vsf'))
                .map(dirent => {
                    const filePath = path.join(patchesDir, dirent.name);
                    const stats = fs.statSync(filePath);
                    return {
                        name: dirent.name.replace('.vsf', ''),
                        mtime: stats.mtime
                    };
                })
                .sort((a, b) => b.mtime.getTime() - a.mtime.getTime()) // Most recent first
                .map(patch => {
                    const shortHash = patch.name.substring(0, 8);
                    const timeStr = patch.mtime.toLocaleString();
                    const isCurrent = currentPatchHash === patch.name;
                    const icon = isCurrent ? '⚪' : '⚫';
                    const label = isCurrent
                        ? `${icon} ${shortHash} (${timeStr}) [CURRENT]`
                        : `${icon} ${shortHash} (${timeStr})`;
                    return new CairnTreeItem(
                        label,
                        vscode.TreeItemCollapsibleState.None,
                        {
                            command: 'cairn.jumpPatch',
                            title: 'Switch to Patch',
                            arguments: [patch.name]
                        },
                        'patch'
                    );
                });

            outputChannel.appendLine(`[PATCHES] Returning ${patches.length} patch items`);
            return patches.length > 0 ? patches : [new CairnTreeItem('No patches yet', vscode.TreeItemCollapsibleState.None)];
        } catch (err) {
            outputChannel.appendLine(`[PATCHES] Error: ${err}`);
            return [new CairnTreeItem('Error reading patches', vscode.TreeItemCollapsibleState.None)];
        }
    }
}

function findCairnBinary(): string {
    const { execSync } = require('child_process');

    // Try multiple locations
    const candidates = [
        'cairn', // In PATH
        path.join(vscode.workspace.workspaceFolders?.[0]?.uri.fsPath || '.', 'target/debug/cairn'),
        path.join(vscode.workspace.workspaceFolders?.[0]?.uri.fsPath || '.', 'target/release/cairn'),
    ];

    for (const candidate of candidates) {
        try {
            // Check if this is the shared memory version by testing for daemon subcommand
            if (fs.existsSync(candidate)) {
                return candidate;
            }
        } catch {
            continue;
        }
    }

    // Default to PATH
    return 'cairn';
}

async function ensureDaemonRunning(): Promise<boolean> {
    if (shmClient.isDaemonRunning()) {
        outputChannel.appendLine('[DAEMON] Already running');
        return true;
    }

    outputChannel.appendLine('[DAEMON] Not running, starting daemon...');

    try {
        const cairnBinary = findCairnBinary();
        outputChannel.appendLine(`[DAEMON] Using binary: ${cairnBinary}`);

        // Start daemon in background
        const { spawn } = require('child_process');
        const daemon = spawn(cairnBinary, ['daemon'], {
            detached: true,
            stdio: 'ignore'
        });
        daemon.unref(); // Allow parent to exit independently

        // Wait a bit for daemon to initialize
        await new Promise(resolve => setTimeout(resolve, 500));

        if (shmClient.isDaemonRunning()) {
            outputChannel.appendLine('[DAEMON] Started successfully');
            return true;
        } else {
            outputChannel.appendLine('[DAEMON] Failed to start - shared memory not found');
            return false;
        }
    } catch (error) {
        outputChannel.appendLine(`[DAEMON] Failed to start: ${error}`);
        return false;
    }
}

export function activate(context: vscode.ExtensionContext) {
    outputChannel = vscode.window.createOutputChannel('Cairn');
    outputChannel.appendLine('=== Cairn extension activated ===');
    outputChannel.show();

    // Initialize shared memory client
    shmClient = new DaemonShmClient();
    outputChannel.appendLine('[INIT] Shared memory client initialized');

    // Auto-start daemon if not running
    ensureDaemonRunning().then(running => {
        if (!running) {
            outputChannel.appendLine('[DAEMON] Warning: Failed to auto-start daemon');
        }
    });

    // Create tree view in sidebar
    const treeProvider = new CairnTreeProvider();
    outputChannel.appendLine('[INIT] Creating tree view');
    const treeView = vscode.window.createTreeView('cairnView', {
        treeDataProvider: treeProvider,
        showCollapseAll: true
    });
    context.subscriptions.push(treeView);
    outputChannel.appendLine('[INIT] Tree view created and registered');

    // Create status bar item for command feedback (must be before commands that use it)
    const statusBar = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
    context.subscriptions.push(statusBar);

    // Register refresh command
    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.refresh', () => {
            outputChannel.appendLine('[CMD] Refresh command called');
            treeProvider.refresh();
        })
    );

    // Watch .cairn/patches directory for changes and auto-refresh
    const workspaceFolders = vscode.workspace.workspaceFolders;
    if (workspaceFolders) {
        const cairnCommitsPattern = new vscode.RelativePattern(
            workspaceFolders[0],
            '.cairn/patches/**/*.vsf'
        );
        const fileWatcher = vscode.workspace.createFileSystemWatcher(cairnCommitsPattern);

        fileWatcher.onDidCreate(() => {
            outputChannel.appendLine('[WATCH] New patch created, refreshing tree');
            treeProvider.refresh();
        });

        fileWatcher.onDidDelete(() => {
            outputChannel.appendLine('[WATCH] Patch deleted, refreshing tree');
            treeProvider.refresh();
        });

        fileWatcher.onDidChange(() => {
            outputChannel.appendLine('[WATCH] Patch modified, refreshing tree');
            treeProvider.refresh();
        });

        context.subscriptions.push(fileWatcher);
        outputChannel.appendLine('[INIT] File watcher registered for .cairn/patches');
    }

    // Poll for daemon events and health check
    let lastHealthCheck = Date.now();
    const eventPollInterval = setInterval(() => {
        try {
            if (shmClient.isDaemonRunning()) {
                const events = shmClient.pollEvents();
                if (events !== 0) {
                    outputChannel.appendLine(`[EVENTS] Received events: 0x${events.toString(16)}`);

                    if (shmClient.hasPatchesChanged(events)) {
                        outputChannel.appendLine('[EVENTS] Patches changed, refreshing tree');
                        treeProvider.refresh();
                    }

                    if (shmClient.hasBuildStarted(events)) {
                        statusBar.text = '$(sync~spin) Building...';
                        statusBar.show();
                    }

                    if (shmClient.hasBuildCompleted(events)) {
                        // Status bar will be updated by command completion
                    }
                }
            } else {
                // Health check: daemon not running, try to restart every 10 seconds
                const now = Date.now();
                if (now - lastHealthCheck > 10000) {
                    lastHealthCheck = now;
                    outputChannel.appendLine('[HEALTH] Daemon not running, attempting auto-restart...');
                    ensureDaemonRunning();
                }
            }
        } catch (error) {
            // Silently ignore polling errors
        }
    }, 500); // Poll every 500ms

    context.subscriptions.push({
        dispose: () => clearInterval(eventPollInterval)
    });

    const cwd = () => vscode.workspace.workspaceFolders?.[0].uri.fsPath || '.';

    // Helper to execute commands in terminal
    const executeInTerminal = (name: string, command: string) => {
        outputChannel.appendLine(`[CMD] ${name} - running in terminal`);

        // Reuse existing terminal or create new one
        let terminal = vscode.window.terminals.find(t => t.name === 'Cairn');
        if (!terminal) {
            terminal = vscode.window.createTerminal({
                name: 'Cairn',
                cwd: cwd()
            });
        }

        terminal.show(true); // Show but don't steal focus
        terminal.sendText(command);

        statusBar.text = `$(terminal) ${name}...`;
        statusBar.show();
        setTimeout(() => statusBar.hide(), 2000);
    };

    // Register cargo commands - use cargo-cairn wrapper for pre-capture snapshots
    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.run', () => {
            executeInTerminal('Run', 'cargo cairn run');
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.runRelease', () => {
            executeInTerminal('Run (Release)', 'cargo cairn run --release');
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.build', () => {
            executeInTerminal('Build', 'cargo cairn build');
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.buildRelease', () => {
            executeInTerminal('Build (Release)', 'cargo cairn build --release');
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.check', () => {
            executeInTerminal('Check', 'cargo cairn check');
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.test', () => {
            executeInTerminal('Test', 'cargo cairn test');
        })
    );

    // Register cairn operations - use daemon for silent execution
    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.clear', async () => {
            outputChannel.appendLine('[CMD] CLEAR command triggered');

            // Ensure daemon is running
            if (!shmClient.isDaemonRunning()) {
                const started = await ensureDaemonRunning();
                if (!started) {
                    statusBar.text = `$(x) Daemon not running`;
                    setTimeout(() => statusBar.hide(), 3000);
                    return;
                }
            }

            statusBar.text = `$(sync~spin) Clearing...`;
            statusBar.show();

            try {
                const response = await shmClient.clear(cwd());

                if (response.status === 'Success') {
                    statusBar.text = `$(check) Cleared`;
                    outputChannel.appendLine(`[CMD] ${response.message}`);
                    treeProvider.refresh();
                    setTimeout(() => statusBar.hide(), 2000);
                } else {
                    statusBar.text = `$(x) Clear failed`;
                    outputChannel.appendLine(`[CMD] ${response.message}`);
                    setTimeout(() => statusBar.hide(), 3000);
                }
            } catch (error) {
                statusBar.text = `$(x) Clear failed`;
                outputChannel.appendLine(`[CMD] Error: ${error}`);
                setTimeout(() => statusBar.hide(), 3000);
            }
        })
    );

    // Jump patch command
    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.jumpPatch', async (patchHash: string) => {
            outputChannel.appendLine(`[CMD] JUMP PATCH command triggered for: ${patchHash}`);

            const shortHash = patchHash.substring(0, 8);
            const cwd = vscode.workspace.workspaceFolders?.[0].uri.fsPath || '.';

            // Update status bar - no blocking dialogs!
            statusBar.text = `$(sync~spin) Switching to ${shortHash}...`;
            statusBar.show();

            try {
                const response = await shmClient.jump(patchHash, cwd);

                if (response.status === 'Success') {
                    statusBar.text = `$(check) Switched to ${shortHash}`;
                    outputChannel.appendLine(`[CMD] ${response.message}`);
                    treeProvider.refresh();
                    setTimeout(() => statusBar.hide(), 3000);
                } else {
                    statusBar.text = `$(x) Failed to switch to ${shortHash}`;
                    outputChannel.appendLine(`[CMD] Jump failed: ${response.message}`);
                    setTimeout(() => statusBar.hide(), 5000);
                }
            } catch (error) {
                statusBar.text = `$(x) Failed to switch to ${shortHash}`;
                outputChannel.appendLine(`[CMD] Jump error: ${error}`);
                setTimeout(() => statusBar.hide(), 5000);
            }
        })
    );

    outputChannel.appendLine('[INIT] All commands registered');
    outputChannel.appendLine('=== Cairn extension ready ===');
}

export function deactivate() {
    if (outputChannel) {
        outputChannel.appendLine('=== Cairn extension deactivated ===');
        outputChannel.dispose();
    }
}
