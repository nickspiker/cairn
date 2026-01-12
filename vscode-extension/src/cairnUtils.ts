import { exec } from 'child_process';
import { promisify } from 'util';
import * as vscode from 'vscode';
import { BinaryManager } from './binaryManager';

const execAsync = promisify(exec);

export async function getGitConfig(key: string): Promise<string> {
    const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
    if (!workspaceFolder) {
        return '';
    }

    try {
        const { stdout } = await execAsync(`git config --get ${key}`, {
            cwd: workspaceFolder.uri.fsPath
        });
        return stdout.trim();
    } catch {
        return '';
    }
}

export async function runCairn(binaryManager: BinaryManager, args: string[]): Promise<string> {
    const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
    if (!workspaceFolder) {
        throw new Error('No workspace folder open');
    }

    const cairnPath = await binaryManager.getCairnPath();

    // Get author info from git config
    const authorName = await getGitConfig('user.name');
    const authorEmail = await getGitConfig('user.email');

    const env = { ...process.env };
    if (authorName) {
        env.CAIRN_AUTHOR_NAME = authorName;
    }
    if (authorEmail) {
        env.CAIRN_AUTHOR_EMAIL = authorEmail;
    }

    const { stdout } = await execAsync(`"${cairnPath}" ${args.join(' ')}`, {
        cwd: workspaceFolder.uri.fsPath,
        env
    });
    return stdout;
}
