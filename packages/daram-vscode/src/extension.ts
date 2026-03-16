import * as vscode from "vscode";
import {
  Executable,
  LanguageClient,
  LanguageClientOptions,
  RevealOutputChannelOn,
  ServerOptions,
  Trace,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  context.subscriptions.push(
    vscode.commands.registerCommand("daram.restartLanguageServer", async () => {
      await stopClient();
      await startClient(context);
    }),
  );

  await startClient(context);
}

export async function deactivate(): Promise<void> {
  await stopClient();
}

async function startClient(context: vscode.ExtensionContext): Promise<void> {
  const config = vscode.workspace.getConfiguration("daram");
  const command = config.get<string>("server.path", "dr");
  const args = config.get<string[]>("server.args", ["lsp"]);
  const outputChannel = vscode.window.createOutputChannel("Daram Language Server");
  context.subscriptions.push(outputChannel);

  const run: Executable = {
    command,
    args,
    options: {
      cwd: workspaceRoot(),
      env: process.env,
    },
    transport: TransportKind.stdio,
  };

  const serverOptions: ServerOptions = {
    run,
    debug: run,
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: "file", language: "daram" },
      { scheme: "untitled", language: "daram" },
    ],
    outputChannel,
    revealOutputChannelOn: RevealOutputChannelOn.Never,
    synchronize: {
      configurationSection: "daram",
      fileEvents: vscode.workspace.createFileSystemWatcher("**/*.dr"),
    },
  };

  client = new LanguageClient("daram", "Daram Language Server", serverOptions, clientOptions);
  client.setTrace(traceLevel(config.get<string>("server.trace", "off")));
  await client.start();
}

async function stopClient(): Promise<void> {
  if (!client) {
    return;
  }
  const current = client;
  client = undefined;
  await current.stop();
}

function workspaceRoot(): string {
  const folder = vscode.workspace.workspaceFolders?.[0];
  return folder ? folder.uri.fsPath : process.cwd();
}

function traceLevel(trace: string): Trace {
  switch (trace) {
    case "messages":
      return Trace.Messages;
    case "verbose":
      return Trace.Verbose;
    default:
      return Trace.Off;
  }
}
