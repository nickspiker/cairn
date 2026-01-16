import * as vscode from 'vscode';
import { CairnProvider } from './cairnProvider';
import { BuildCommandsProvider } from './buildCommandsProvider';
import { BinaryManager } from './binaryManager';
import { runCairn } from './cairnUtils';
import { exec } from 'child_process';
import { promisify } from 'util';

const execAsync = promisify(exec);

let cairnProvider: CairnProvider;
let buildCommandsProvider: BuildCommandsProvider;
let binaryManager: BinaryManager;
let cairnTreeView: vscode.TreeView<any>;

export function activate(context: vscode.ExtensionContext) {
    console.log('Cairn extension activated');

    // Initialize binary manager
    binaryManager = new BinaryManager(context);

    // Initialize cairn provider (tree view)
    cairnProvider = new CairnProvider(context, binaryManager);
    cairnTreeView = vscode.window.createTreeView('cairnHistory', {
        treeDataProvider: cairnProvider
    });

    // Initialize build commands provider
    buildCommandsProvider = new BuildCommandsProvider();
    vscode.window.registerTreeDataProvider('cairnBuildCommands', buildCommandsProvider);

    // Register commands
    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.init', async () => {
            try {
                await runCairn(binaryManager, ['init']);
                cairnProvider.refresh();
                vscode.window.showInformationMessage('✓ Cairn initialized');
            } catch (error) {
                vscode.window.showErrorMessage(`Failed to initialize cairn: ${error}`);
            }
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.list', async () => {
            try {
                const output = await runCairn(binaryManager, ['list']);
                const outputChannel = vscode.window.createOutputChannel('Cairn');
                outputChannel.clear();
                outputChannel.appendLine(output);
                outputChannel.show();
            } catch (error) {
                vscode.window.showErrorMessage(`Failed to list patches: ${error}`);
            }
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.rollback', async (patchItem) => {
            if (patchItem && patchItem.patchId) {
                await rollbackToPatch(patchItem.patchId);
            } else {
                // Show quick pick if no patch selected
                const patches = await cairnProvider.getPatches();
                if (patches.length === 0) {
                    vscode.window.showInformationMessage('No patches available');
                    return;
                }

                const items = patches.map(p => ({
                    label: p.mnemonic,
                    description: p.marker,
                    patchId: p.id
                }));

                const selected = await vscode.window.showQuickPick(items, {
                    placeHolder: 'Select a patch to rollback to'
                });

                if (selected) {
                    await rollbackToPatch(selected.patchId);
                }
            }
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.showPatch', async (patchItem) => {
            if (patchItem && patchItem.patchId) {
                await showPatchDetails(patchItem.patchId);
            }
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.refresh', () => {
            cairnProvider.refresh();
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.runBuildCommand', async (buildCommandItem) => {
            if (buildCommandItem && buildCommandItem.cargoCommand) {
                await runBuildCommand(buildCommandItem.cargoCommand);
            }
        })
    );

    // Watch for .cairn/state.vsf changes in all workspace folders
    const stateWatcher = vscode.workspace.createFileSystemWatcher('**/.cairn/state.vsf');
    stateWatcher.onDidChange(() => cairnProvider.refresh());
    stateWatcher.onDidCreate(() => cairnProvider.refresh());
    stateWatcher.onDidDelete(() => cairnProvider.refresh());
    context.subscriptions.push(stateWatcher);

    // Refresh patch history when switching between files in different projects
    context.subscriptions.push(
        vscode.window.onDidChangeActiveTextEditor(() => {
            cairnProvider.refreshIfCargoRootChanged();
        })
    );

    // Initial refresh
    cairnProvider.refresh();
}

export function deactivate() {
    // Cleanup if needed
}

async function rollbackToPatch(patchId: string): Promise<void> {
    try {
        await runCairn(binaryManager, ['rollback', patchId]);
        cairnProvider.refresh();
        vscode.window.showInformationMessage(`✓ Rolled back to ${patchId}`);
    } catch (error) {
        vscode.window.showErrorMessage(`Failed to rollback: ${error}`);
    }
}

async function showPatchDetails(patchId: string): Promise<void> {
    try {
        const output = await runCairn(binaryManager, ['show', patchId]);
        const outputChannel = vscode.window.createOutputChannel('Cairn');
        outputChannel.clear();
        outputChannel.appendLine(output);
        outputChannel.show();
    } catch (error) {
        vscode.window.showErrorMessage(`Failed to show patch: ${error}`);
    }
}

async function runBuildCommand(cargoCommand: string): Promise<void> {
    // Use the active editor's workspace folder, or fall back to first workspace
    const activeEditor = vscode.window.activeTextEditor;
    let workspaceFolder = activeEditor
        ? vscode.workspace.getWorkspaceFolder(activeEditor.document.uri)
        : undefined;

    if (!workspaceFolder) {
        workspaceFolder = vscode.workspace.workspaceFolders?.[0];
    }

    if (!workspaceFolder) {
        vscode.window.showErrorMessage('No workspace folder open');
        return;
    }

    try {
        // Ensure cargo-cairn is installed (downloads from GitHub if needed)
        await binaryManager.ensureCargoCairnInstalled();

        // Show output in dedicated output channel
        const outputChannel = vscode.window.createOutputChannel('Cairn Build');
        outputChannel.clear();
        outputChannel.show();

        outputChannel.appendLine(`Running: cargo cairn ${cargoCommand}\n`);

        // Run cargo cairn command
        const { stdout, stderr } = await execAsync(`cargo cairn ${cargoCommand}`, {
            cwd: workspaceFolder.uri.fsPath
        });

        if (stdout) {
            outputChannel.appendLine(stdout);
        }
        if (stderr) {
            outputChannel.appendLine(stderr);
        }

        // Refresh the patch history after build
        cairnProvider.refresh();

        // Wait a bit for the tree to update, then select the head patch
        setTimeout(async () => {
            const patches = await cairnProvider.getPatches();
            const headPatch = patches.find(p => p.marker.includes('CURRENT'));
            if (headPatch && cairnTreeView) {
                const items = await cairnProvider.getChildren();
                const headItem = items.find(item => item.patchId === headPatch.id);
                if (headItem) {
                    cairnTreeView.reveal(headItem, { select: true, focus: false });
                }
            }
        }, 100);

        vscode.window.showInformationMessage(`✓ cargo cairn ${cargoCommand} completed`);
    } catch (error: any) {
        const outputChannel = vscode.window.createOutputChannel('Cairn Build');
        outputChannel.appendLine(`Error running cargo cairn ${cargoCommand}:`);
        outputChannel.appendLine('');
        if (error.stdout) {
            outputChannel.appendLine('STDOUT:');
            outputChannel.appendLine(error.stdout);
        }
        if (error.stderr) {
            outputChannel.appendLine('STDERR:');
            outputChannel.appendLine(error.stderr);
        }
        if (error.message) {
            outputChannel.appendLine('ERROR MESSAGE:');
            outputChannel.appendLine(error.message);
        }
        outputChannel.show();
        vscode.window.showErrorMessage(`Failed to run cargo cairn ${cargoCommand}`);
    }
}
