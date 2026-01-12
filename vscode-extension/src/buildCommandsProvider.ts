import * as vscode from 'vscode';

export interface BuildCommand {
    label: string;
    command: string;
    icon: string;
}

export class BuildCommandsProvider implements vscode.TreeDataProvider<BuildCommandItem> {
    private _onDidChangeTreeData: vscode.EventEmitter<BuildCommandItem | undefined | null | void> = new vscode.EventEmitter<BuildCommandItem | undefined | null | void>();
    readonly onDidChangeTreeData: vscode.Event<BuildCommandItem | undefined | null | void> = this._onDidChangeTreeData.event;

    constructor() {}

    refresh(): void {
        this._onDidChangeTreeData.fire();
    }

    getTreeItem(element: BuildCommandItem): vscode.TreeItem {
        return element;
    }

    async getChildren(element?: BuildCommandItem): Promise<BuildCommandItem[]> {
        if (element) {
            return [];
        }

        // Read build commands from settings
        const config = vscode.workspace.getConfiguration('cairn');
        const commands = config.get<BuildCommand[]>('buildCommands') || [];

        return commands.map(cmd => new BuildCommandItem(cmd));
    }
}

class BuildCommandItem extends vscode.TreeItem {
    constructor(public readonly buildCommand: BuildCommand) {
        super(buildCommand.label, vscode.TreeItemCollapsibleState.None);

        this.tooltip = `cargo cairn ${buildCommand.command}`;
        this.contextValue = 'buildCommand';

        // Set icon from command configuration
        this.iconPath = new vscode.ThemeIcon(buildCommand.icon);

        // Make it clickable
        this.command = {
            command: 'cairn.runBuildCommand',
            title: 'Run Build Command',
            arguments: [this]
        };
    }

    get cargoCommand(): string {
        return this.buildCommand.command;
    }
}
