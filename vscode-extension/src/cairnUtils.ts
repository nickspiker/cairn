import { exec } from 'child_process';
import { promisify } from 'util';
import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import { BinaryManager } from './binaryManager';

const execAsync = promisify(exec);

/**
 * Get the workspace folder for the currently active Rust project
 * Falls back to first workspace folder if no active editor
 */
export function getActiveWorkspaceFolder(): vscode.WorkspaceFolder | undefined {
    const activeEditor = vscode.window.activeTextEditor;
    if (activeEditor) {
        const workspaceFolder = vscode.workspace.getWorkspaceFolder(activeEditor.document.uri);
        if (workspaceFolder) {
            return workspaceFolder;
        }
    }
    return vscode.workspace.workspaceFolders?.[0];
}

/**
 * Find the Cargo project root for a given file path
 * Returns the directory containing Cargo.toml, or undefined if not found
 */
export function findCargoRoot(filePath: string): string | undefined {
    let currentDir = path.dirname(filePath);
    const root = path.parse(currentDir).root;

    while (currentDir !== root) {
        const cargoTomlPath = path.join(currentDir, 'Cargo.toml');
        if (fs.existsSync(cargoTomlPath)) {
            return currentDir;
        }
        currentDir = path.dirname(currentDir);
    }

    return undefined;
}

/**
 * Get the Cargo project root for the active editor
 * Falls back to workspace folder if no Cargo.toml found
 */
export function getActiveCargoRoot(): string | undefined {
    const activeEditor = vscode.window.activeTextEditor;
    if (activeEditor) {
        const cargoRoot = findCargoRoot(activeEditor.document.uri.fsPath);
        if (cargoRoot) {
            return cargoRoot;
        }
    }

    const workspaceFolder = getActiveWorkspaceFolder();
    return workspaceFolder?.uri.fsPath;
}

export async function getGitConfig(key: string, cwd?: string): Promise<string> {
    const workingDir = cwd || getActiveWorkspaceFolder()?.uri.fsPath;
    if (!workingDir) {
        return '';
    }

    try {
        const { stdout } = await execAsync(`git config --get ${key}`, {
            cwd: workingDir
        });
        return stdout.trim();
    } catch {
        return '';
    }
}

export async function runCairn(binaryManager: BinaryManager, args: string[], cwd?: string): Promise<string> {
    const workingDir = cwd || getActiveCargoRoot();
    if (!workingDir) {
        throw new Error('No workspace folder open');
    }

    const cairnPath = await binaryManager.getCairnPath();

    // Get author info from git config
    const authorName = await getGitConfig('user.name', workingDir);
    const authorEmail = await getGitConfig('user.email', workingDir);

    const env = { ...process.env };
    if (authorName) {
        env.CAIRN_AUTHOR_NAME = authorName;
    }
    if (authorEmail) {
        env.CAIRN_AUTHOR_EMAIL = authorEmail;
    }

    const { stdout } = await execAsync(`"${cairnPath}" ${args.join(' ')}`, {
        cwd: workingDir,
        env
    });
    return stdout;
}
