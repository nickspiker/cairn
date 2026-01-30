import * as vscode from 'vscode';
import * as fs from 'fs';
import * as path from 'path';

let outputChannel: vscode.OutputChannel;

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

export function activate(context: vscode.ExtensionContext) {
    outputChannel = vscode.window.createOutputChannel('Cairn');
    outputChannel.appendLine('=== Cairn extension activated ===');
    outputChannel.show();

    // Create tree view in sidebar
    const treeProvider = new CairnTreeProvider();
    outputChannel.appendLine('[INIT] Creating tree view');
    const treeView = vscode.window.createTreeView('cairnView', {
        treeDataProvider: treeProvider,
        showCollapseAll: true
    });
    context.subscriptions.push(treeView);
    outputChannel.appendLine('[INIT] Tree view created and registered');

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

    // Register build commands
    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.run', async () => {
            outputChannel.appendLine('[CMD] RUN command triggered');
            const task = new vscode.Task(
                { type: 'shell' },
                vscode.TaskScope.Workspace,
                'Cairn Run',
                'cairn',
                new vscode.ShellExecution('cargo cairn run')
            );
            task.presentationOptions = {
                reveal: vscode.TaskRevealKind.Always,
                panel: vscode.TaskPanelKind.Dedicated
            };
            outputChannel.appendLine('[CMD] Executing run task');
            await vscode.tasks.executeTask(task);
            outputChannel.appendLine('[CMD] Run task started');
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.runRelease', async () => {
            outputChannel.appendLine('[CMD] RUN RELEASE command triggered');
            const task = new vscode.Task(
                { type: 'shell' },
                vscode.TaskScope.Workspace,
                'Cairn Run (Release)',
                'cairn',
                new vscode.ShellExecution('cargo cairn run --release')
            );
            task.presentationOptions = {
                reveal: vscode.TaskRevealKind.Always,
                panel: vscode.TaskPanelKind.Dedicated
            };
            await vscode.tasks.executeTask(task);
            outputChannel.appendLine('[CMD] Run release task started');
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.build', async () => {
            outputChannel.appendLine('[CMD] BUILD command triggered');
            const task = new vscode.Task(
                { type: 'shell' },
                vscode.TaskScope.Workspace,
                'Cairn Build',
                'cairn',
                new vscode.ShellExecution('cargo cairn build')
            );
            task.presentationOptions = {
                reveal: vscode.TaskRevealKind.Always,
                panel: vscode.TaskPanelKind.Dedicated
            };
            outputChannel.appendLine('[CMD] Executing build task');
            await vscode.tasks.executeTask(task);
            outputChannel.appendLine('[CMD] Build task started, scheduling refresh');
            // Refresh patches after build completes
            setTimeout(() => {
                outputChannel.appendLine('[CMD] Refreshing after build timeout');
                treeProvider.refresh();
            }, 1000);
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.buildRelease', async () => {
            outputChannel.appendLine('[CMD] BUILD RELEASE command triggered');
            const task = new vscode.Task(
                { type: 'shell' },
                vscode.TaskScope.Workspace,
                'Cairn Build (Release)',
                'cairn',
                new vscode.ShellExecution('cargo cairn build --release')
            );
            task.presentationOptions = {
                reveal: vscode.TaskRevealKind.Always,
                panel: vscode.TaskPanelKind.Dedicated
            };
            await vscode.tasks.executeTask(task);
            outputChannel.appendLine('[CMD] Build release task started, scheduling refresh');
            setTimeout(() => treeProvider.refresh(), 1000);
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.check', async () => {
            outputChannel.appendLine('[CMD] CHECK command triggered');
            const task = new vscode.Task(
                { type: 'shell' },
                vscode.TaskScope.Workspace,
                'Cairn Check',
                'cairn',
                new vscode.ShellExecution('cargo cairn check')
            );
            task.presentationOptions = {
                reveal: vscode.TaskRevealKind.Always,
                panel: vscode.TaskPanelKind.Dedicated
            };
            await vscode.tasks.executeTask(task);
            outputChannel.appendLine('[CMD] Check task started');
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.test', async () => {
            outputChannel.appendLine('[CMD] TEST command triggered');
            const task = new vscode.Task(
                { type: 'shell' },
                vscode.TaskScope.Workspace,
                'Cairn Test',
                'cairn',
                new vscode.ShellExecution('cargo cairn test')
            );
            task.presentationOptions = {
                reveal: vscode.TaskRevealKind.Always,
                panel: vscode.TaskPanelKind.Dedicated
            };
            await vscode.tasks.executeTask(task);
            outputChannel.appendLine('[CMD] Test task started, scheduling refresh');
            setTimeout(() => treeProvider.refresh(), 1000);
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.clear', async () => {
            outputChannel.appendLine('[CMD] CLEAR command triggered');

            // Confirm with user first
            const answer = await vscode.window.showWarningMessage(
                'This will DELETE the .cairn directory and ALL patch history. This action cannot be undone.',
                { modal: true },
                'Delete'
            );

            if (answer !== 'Delete') {
                outputChannel.appendLine('[CMD] Clear cancelled by user');
                return;
            }

            const task = new vscode.Task(
                { type: 'shell' },
                vscode.TaskScope.Workspace,
                'Cairn Clear',
                'cairn',
                new vscode.ShellExecution('echo y | cairn clear')
            );
            task.presentationOptions = {
                reveal: vscode.TaskRevealKind.Always,
                panel: vscode.TaskPanelKind.Dedicated
            };
            await vscode.tasks.executeTask(task);
            outputChannel.appendLine('[CMD] Clear task started');

            // Refresh the tree after clearing
            setTimeout(() => treeProvider.refresh(), 1000);
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.jumpPatch', async (patchHash: string) => {
            outputChannel.appendLine(`[CMD] JUMP PATCH command triggered for: ${patchHash}`);

            // Confirm with user
            const answer = await vscode.window.showWarningMessage(
                `Switch to patch ${patchHash.substring(0, 8)}? This will restore your workspace to that patch state.`,
                'Switch', 'Cancel'
            );

            if (answer !== 'Switch') {
                outputChannel.appendLine('[CMD] Jump cancelled by user');
                return;
            }

            // Use cairn jump command to switch patches
            const task = new vscode.Task(
                { type: 'shell' },
                vscode.TaskScope.Workspace,
                'Switch Patch',
                'cairn',
                new vscode.ShellExecution(`cairn jump ${patchHash}`)
            );
            task.presentationOptions = {
                reveal: vscode.TaskRevealKind.Always,
                panel: vscode.TaskPanelKind.Dedicated
            };
            await vscode.tasks.executeTask(task);
            outputChannel.appendLine('[CMD] Jump task started');

            // Refresh tree after jump
            setTimeout(() => treeProvider.refresh(), 1000);
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
