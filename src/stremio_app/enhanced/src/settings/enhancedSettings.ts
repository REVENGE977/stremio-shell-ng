import { postEnhancedMessage } from '../utils/dom';
import { EnhancedData } from '../types';
import { getActiveDispatch, getActiveGetState } from '../core/permanentCore';
import { modalManager } from '../components/modals/modalManager';

interface TransferResult {
    ok: boolean;
    cancelled?: boolean;
    path?: string;
    error?: string;
    pluginSettings?: Record<string, any> | null;
}

let discordRpcEnabled = true;
let handlersInitialized = false;

const pendingExports = new Map<string, (result: TransferResult) => void>();
const pendingImports = new Map<string, (result: TransferResult) => void>();

export function isDiscordRpcEnabled(): boolean {
    return discordRpcEnabled;
}

export function syncDiscordRpcToggle(): void {
    const toggle = document.getElementById('discordrpcToggle');
    if (!toggle) return;
    toggle.classList.toggle('checked', discordRpcEnabled);
    toggle.setAttribute('aria-checked', discordRpcEnabled ? 'true' : 'false');
}

function syncDiscordRpcToCore(enabled: boolean): void {
    try {
        const getState = getActiveGetState();
        const dispatch = getActiveDispatch();
        const settings = getState?.('ctx')?.profile?.settings;
        if (settings && dispatch && settings.discordRpcEnabled !== enabled) {
            dispatch({
                action: 'Ctx',
                args: {
                    action: 'UpdateSettings',
                    args: {
                        ...settings,
                        discordRpcEnabled: enabled
                    }
                }
            });
        }
    } catch (_) {}
}

export function updateEnhancedSettingsData(data: EnhancedData): void {
    if (typeof data.discordRpc === 'boolean') {
        discordRpcEnabled = data.discordRpc;
        syncDiscordRpcToggle();
    }
}

export function setDiscordRpc(enabled: boolean): void {
    discordRpcEnabled = enabled;
    syncDiscordRpcToggle();
    postEnhancedMessage('setDiscordRpc', { enabled });
    syncDiscordRpcToCore(enabled);
}

export function setupSettingsTransferHandlers(): void {
    if (handlersInitialized) return;
    handlersInitialized = true;

    (window as any).__receiveExportSettingsResult = function (id: string, result: TransferResult) {
        const resolve = pendingExports.get(id);
        if (resolve) {
            pendingExports.delete(id);
            resolve(result);
        }
    };

    (window as any).__receiveImportSettingsResult = function (id: string, result: TransferResult) {
        const resolve = pendingImports.get(id);
        if (resolve) {
            pendingImports.delete(id);
            resolve(result);
        }
    };
}

function collectPluginSettingsFromStorage(): Record<string, any> {
    const pluginSettings: Record<string, any> = {};
    try {
        for (let i = 0; i < localStorage.length; i++) {
            const key = localStorage.key(i);
            if (!key || !key.startsWith('plugin_setting:')) continue;
            const raw = localStorage.getItem(key);
            if (raw === null) continue;
            try {
                pluginSettings[key] = JSON.parse(raw);
            } catch (_) {
                pluginSettings[key] = raw;
            }
        }
    } catch (_) {}
    return pluginSettings;
}

function applyPluginSettingsToStorage(pluginSettings: Record<string, any> | null | undefined): void {
    if (!pluginSettings || typeof pluginSettings !== 'object') return;
    try {
        for (const [key, value] of Object.entries(pluginSettings)) {
            if (!key.startsWith('plugin_setting:')) continue;
            try {
                localStorage.setItem(key, JSON.stringify(value));
            } catch (_) {
                localStorage.setItem(key, String(value));
            }
        }
    } catch (_) {}
}

export function exportEnhancedSetup(): Promise<void> {
    setupSettingsTransferHandlers();
    const id = `exp_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`;
    const pluginSettings = collectPluginSettingsFromStorage();

    return new Promise((resolve) => {
        pendingExports.set(id, (result) => {
            if (result?.ok) {
                modalManager.showToast(
                    'settings-export',
                    'Setup Exported',
                    `Saved setup to ${result.path || 'settings file'}.`,
                    'success'
                );
            } else if (!result?.cancelled) {
                modalManager.showToast(
                    'settings-export-err',
                    'Export Failed',
                    result?.error || 'Could not export settings.',
                    'fail'
                );
            }
            resolve();
        });
        postEnhancedMessage('exportSettings', { id, pluginSettings });
    });
}

export function importEnhancedSetup(): Promise<void> {
    setupSettingsTransferHandlers();
    const id = `imp_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`;

    return new Promise((resolve) => {
        pendingImports.set(id, (result) => {
            if (result?.ok) {
                applyPluginSettingsToStorage(result.pluginSettings);
                syncDiscordRpcToggle();
                syncDiscordRpcToCore(discordRpcEnabled);
                modalManager.showToast(
                    'settings-import',
                    'Setup Imported',
                    'Settings imported successfully. Reload if plugin changes require restart.',
                    'success'
                );
            } else if (!result?.cancelled) {
                modalManager.showToast(
                    'settings-import-err',
                    'Import Failed',
                    result?.error || 'Could not import settings.',
                    'fail'
                );
            }
            resolve();
        });
        postEnhancedMessage('importSettings', { id });
    });
}
