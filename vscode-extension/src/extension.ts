import * as vscode from 'vscode';
import { CairnProvider } from './cairnProvider';
import { BuildTaskMonitor } from './buildMonitor';
import { BinaryManager } from './binaryManager';

let cairnProvider: CairnProvider;
let buildMonitor: BuildTaskMonitor;
let binaryManager: BinaryManager;

export function activate(context: vscode.ExtensionContext) {
    console.log('Cairn extension activated');

    // Initialize binary manager
    binaryManager = new BinaryManager(context);

    // Initialize cairn provider (tree view)
    cairnProvider = new CairnProvider(context, binaryManager);
    vscode.window.registerTreeDataProvider('cairnHistory', cairnProvider);

    // Initialize build task monitor
    buildMonitor = new BuildTaskMonitor(context, cairnProvider, binaryManager);

    // Register commands
    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.init', async () => {
            await runCairnCommand(['init']);
            cairnProvider.refresh();
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('cairn.list', async () => {
            await runCairnCommand(['list']);
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

    // Initial refresh
    cairnProvider.refresh();
}

export function deactivate() {
    if (buildMonitor) {
        buildMonitor.dispose();
    }
}

async function runCairnCommand(args: string[]): Promise<void> {
    const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
    if (!workspaceFolder) {
        vscode.window.showErrorMessage('No workspace folder open');
        return;
    }

    try {
        const cairnPath = await binaryManager.getCairnPath();

        const terminal = vscode.window.createTerminal({
            name: 'Cairn',
            cwd: workspaceFolder.uri.fsPath
        });

        terminal.show();
        terminal.sendText(`"${cairnPath}" ${args.join(' ')}`);
    } catch (error) {
        vscode.window.showErrorMessage(`Failed to get cairn binary: ${error}`);
    }
}

async function rollbackToPatch(patchId: string): Promise<void> {
    const confirmed = await vscode.window.showWarningMessage(
        `Rollback to patch ${patchId}? This will overwrite your working directory.`,
        { modal: true },
        'Rollback'
    );

    if (confirmed) {
        await runCairnCommand(['rollback', patchId]);
        cairnProvider.refresh();
        vscode.window.showInformationMessage(`Rolled back to patch ${patchId}`);
    }
}

async function showPatchDetails(patchId: string): Promise<void> {
    await runCairnCommand(['show', patchId]);
}
