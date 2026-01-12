import * as vscode from 'vscode';
import * as path from 'path';
import { execSync } from 'child_process';
import { BinaryManager } from './binaryManager';

export interface PatchInfo {
    id: string;
    index: number;
    mnemonic: string;
    marker: string;
}

export class CairnProvider implements vscode.TreeDataProvider<PatchTreeItem> {
    private _onDidChangeTreeData: vscode.EventEmitter<PatchTreeItem | undefined | null | void> = new vscode.EventEmitter<PatchTreeItem | undefined | null | void>();
    readonly onDidChangeTreeData: vscode.Event<PatchTreeItem | undefined | null | void> = this._onDidChangeTreeData.event;

    constructor(private context: vscode.ExtensionContext, private binaryManager: BinaryManager) {}

    refresh(): void {
        this._onDidChangeTreeData.fire();
    }

    getTreeItem(element: PatchTreeItem): vscode.TreeItem {
        return element;
    }

    async getChildren(element?: PatchTreeItem): Promise<PatchTreeItem[]> {
        if (element) {
            return [];
        }

        const patches = await this.getPatches();
        return patches.map(p => new PatchTreeItem(p));
    }

    async getPatches(): Promise<PatchInfo[]> {
        const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
        if (!workspaceFolder) {
            return [];
        }

        const cairnDir = path.join(workspaceFolder.uri.fsPath, '.cairn');
        try {
            // Check if .cairn exists
            const fs = require('fs');
            if (!fs.existsSync(cairnDir)) {
                return [];
            }

            // Run cairn list and parse output
            const cairnPath = await this.binaryManager.getCairnPath();
            const output = execSync(`"${cairnPath}" list`, {
                cwd: workspaceFolder.uri.fsPath,
                encoding: 'utf8'
            });

            // Parse the output
            // Format: "  #0   mnemonic-words-here (CURRENT, NEWEST)"
            const patches: PatchInfo[] = [];
            const lines = output.split('\n');

            for (const line of lines) {
                const match = line.match(/^\s*#(\d+)\s+([a-z-]+)(\s+\(.*\))?$/);
                if (match) {
                    patches.push({
                        id: match[2],
                        index: parseInt(match[1]),
                        mnemonic: match[2],
                        marker: match[3] ? match[3].trim() : ''
                    });
                }
            }

            return patches;
        } catch (error) {
            console.error('Failed to get patches:', error);
            return [];
        }
    }
}

class PatchTreeItem extends vscode.TreeItem {
    constructor(public readonly patch: PatchInfo) {
        super(patch.mnemonic, vscode.TreeItemCollapsibleState.None);

        this.description = patch.marker;
        this.tooltip = `#${patch.index} ${patch.mnemonic} ${patch.marker}`;
        this.contextValue = 'patch';

        // Use different icons for current vs other patches
        if (patch.marker.includes('CURRENT')) {
            this.iconPath = new vscode.ThemeIcon('circle-filled', new vscode.ThemeColor('charts.green'));
        } else {
            this.iconPath = new vscode.ThemeIcon('circle-outline');
        }

        // Store patch ID for commands
        this.command = {
            command: 'cairn.showPatch',
            title: 'Show Patch',
            arguments: [this]
        };
    }

    get patchId(): string {
        return this.patch.id;
    }
}
