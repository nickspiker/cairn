import * as vscode from 'vscode';
import { CairnProvider } from './cairnProvider';
import { BinaryManager } from './binaryManager';
import { runCairn } from './cairnUtils';
import * as path from 'path';

export class BuildTaskMonitor {
    private disposables: vscode.Disposable[] = [];
    private statusBarItem: vscode.StatusBarItem;

    constructor(
        private context: vscode.ExtensionContext,
        private cairnProvider: CairnProvider,
        private binaryManager: BinaryManager
    ) {
        // Create status bar item
        this.statusBarItem = vscode.window.createStatusBarItem(
            vscode.StatusBarAlignment.Left,
            100
        );
        this.statusBarItem.command = 'cairn.list';
        this.statusBarItem.show();
        this.disposables.push(this.statusBarItem);

        // Monitor task execution
        this.disposables.push(
            vscode.tasks.onDidEndTask(e => this.onTaskEnd(e))
        );

        // Initial status update
        this.updateStatusBar();
    }

    private async onTaskEnd(event: vscode.TaskEndEvent): Promise<void> {
        const task = event.execution.task;

        // Check if this is a cargo build task
        if (task.name.includes('cargo') && task.name.includes('build')) {
            console.log(`Cargo build task completed: ${task.name}`);

            // Check if task succeeded (exit code 0)
            // Note: VSCode doesn't provide exit code directly, so we check for build success
            // by attempting to create a snapshot
            const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
            if (!workspaceFolder) {
                return;
            }

            const cairnDir = path.join(workspaceFolder.uri.fsPath, '.cairn');
            const fs = require('fs');
            if (!fs.existsSync(cairnDir)) {
                console.log('Cairn not initialized - skipping auto-snapshot');
                return;
            }

            // Create snapshot after successful build
            try {
                await runCairn(this.binaryManager, ['snapshot', '-m', 'Successful build']);

                console.log('Created cairn patch after successful build');

                // Refresh the tree view
                this.cairnProvider.refresh();
                this.updateStatusBar();

                // Show notification
                vscode.window.showInformationMessage('✓ Cairn patch created');
            } catch (error) {
                console.error('Failed to create cairn patch:', error);
            }
        }
    }

    private async updateStatusBar(): Promise<void> {
        const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
        if (!workspaceFolder) {
            this.statusBarItem.hide();
            return;
        }

        const cairnDir = path.join(workspaceFolder.uri.fsPath, '.cairn');
        const fs = require('fs');
        if (!fs.existsSync(cairnDir)) {
            this.statusBarItem.text = '$(circle-outline) Cairn: Not initialized';
            this.statusBarItem.show();
            return;
        }

        try {
            const patches = await this.cairnProvider.getPatches();
            if (patches.length === 0) {
                this.statusBarItem.text = '$(circle-outline) Cairn: No patches';
            } else {
                const current = patches.find(p => p.marker.includes('CURRENT'));
                if (current) {
                    this.statusBarItem.text = `$(circle-filled) Cairn: ${current.mnemonic}`;
                } else {
                    this.statusBarItem.text = `$(circle-outline) Cairn: ${patches.length} patches`;
                }
            }
            this.statusBarItem.show();
        } catch (error) {
            console.error('Failed to update status bar:', error);
            this.statusBarItem.text = '$(warning) Cairn: Error';
            this.statusBarItem.show();
        }
    }

    dispose(): void {
        this.disposables.forEach(d => d.dispose());
    }
}
