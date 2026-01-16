import * as vscode from 'vscode';
import * as path from 'path';
import { BinaryManager } from './binaryManager';
import { runCairn, getActiveCargoRoot } from './cairnUtils';

export interface PatchInfo {
    id: string;
    index: number;
    mnemonic: string;
    marker: string;
}

export class CairnProvider implements vscode.TreeDataProvider<PatchTreeItem> {
    private _onDidChangeTreeData: vscode.EventEmitter<PatchTreeItem | undefined | null | void> = new vscode.EventEmitter<PatchTreeItem | undefined | null | void>();
    readonly onDidChangeTreeData: vscode.Event<PatchTreeItem | undefined | null | void> = this._onDidChangeTreeData.event;
    private currentCargoRoot: string | undefined;

    constructor(private context: vscode.ExtensionContext, private binaryManager: BinaryManager) {}

    refresh(): void {
        this._onDidChangeTreeData.fire();
    }

    /**
     * Refresh if the active Cargo root has changed
     */
    refreshIfCargoRootChanged(): void {
        const newCargoRoot = getActiveCargoRoot();
        if (newCargoRoot !== this.currentCargoRoot) {
            this.currentCargoRoot = newCargoRoot;
            this.refresh();
        }
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
        const cargoRoot = getActiveCargoRoot();
        if (!cargoRoot) {
            return [];
        }

        this.currentCargoRoot = cargoRoot;
        const cairnDir = path.join(cargoRoot, '.cairn');

        try {
            // Check if .cairn exists
            const fs = require('fs');
            if (!fs.existsSync(cairnDir)) {
                return [];
            }

            // Run cairn list in the active Cargo root
            const output = await runCairn(this.binaryManager, ['list'], cargoRoot);

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

        // White dot for current, black dot for others
        if (patch.marker.includes('CURRENT')) {
            this.iconPath = new vscode.ThemeIcon('circle-filled');
        } else {
            this.iconPath = new vscode.ThemeIcon('circle-filled', new vscode.ThemeColor('terminal.ansiBlack'));
            // Click to rollback (only for non-current patches)
            this.command = {
                command: 'cairn.rollback',
                title: 'Rollback to Patch',
                arguments: [this]
            };
        }
    }

    get patchId(): string {
        return this.patch.id;
    }
}
