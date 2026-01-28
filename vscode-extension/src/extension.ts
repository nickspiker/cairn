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

    refresh(): void {
        outputChannel.appendLine('[REFRESH] Tree view refresh triggered');
        this._onDidChangeTreeData.fire();
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
                new CairnTreeItem('🗑 Clean', vscode.TreeItemCollapsibleState.None, {
                    command: 'cairn.clean',
                    title: 'Clean',
                }),
            ];
        } else if (element.contextValue === 'patches') {
            // List patches from .cairn/patches directory
            outputChannel.appendLine('[PATCHES] Loading patches list');
            return this.getPatches();
        }
        return [];
    }

    private getPatches(): CairnTreeItem[] {
        const workspaceFolders = vscode.workspace.workspaceFolders;
        if (!workspaceFolders) {
            outputChannel.appendLine('[PATCHES] No workspace folders');
            return [];
        }

        const cairnDir = path.join(workspaceFolders[0].uri.fsPath, '.cairn', 'patches');
        outputChannel.appendLine(`[PATCHES] Reading from: ${cairnDir}`);

        if (!fs.existsSync(cairnDir)) {
            outputChannel.appendLine('[PATCHES] Directory does not exist');
            return [new CairnTreeItem('No patches yet', vscode.TreeItemCollapsibleState.None)];
        }

        try {
            const files = fs.readdirSync(cairnDir, { withFileTypes: true });
            outputChannel.appendLine(`[PATCHES] Found ${files.length} files`);

            const patches = files
                .filter(dirent => dirent.isFile())
                .map(dirent => dirent.name)
                .sort()
                .reverse() // Most recent first
                .map(file => {
                    return new CairnTreeItem(
                        `📦 ${file}`,
                        vscode.TreeItemCollapsibleState.None,
                        {
                            command: 'cairn.showPatch',
                            title: 'Show Patch',
                            arguments: [file]
                        }
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
        vscode.commands.registerCommand('cairn.clean', async () => {
            outputChannel.appendLine('[CMD] CLEAN command triggered');
            const task = new vscode.Task(
                { type: 'shell' },
                vscode.TaskScope.Workspace,
                'Cairn Clean',
                'cairn',
                new vscode.ShellExecution('cargo cairn clean')
            );
            task.presentationOptions = {
                reveal: vscode.TaskRevealKind.Always,
                panel: vscode.TaskPanelKind.Dedicated
            };
            await vscode.tasks.executeTask(task);
            outputChannel.appendLine('[CMD] Clean task started');
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.showPatch', async (patchName: string) => {
            outputChannel.appendLine(`[CMD] SHOW PATCH command triggered for: ${patchName}`);
            const task = new vscode.Task(
                { type: 'shell' },
                vscode.TaskScope.Workspace,
                'Cairn Show Patch',
                'cairn',
                new vscode.ShellExecution(`cargo cairn show ${patchName}`)
            );
            task.presentationOptions = {
                reveal: vscode.TaskRevealKind.Always,
                panel: vscode.TaskPanelKind.Dedicated
            };
            await vscode.tasks.executeTask(task);
            outputChannel.appendLine('[CMD] Show patch task started');
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
