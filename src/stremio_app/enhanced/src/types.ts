export interface MetaData {
    name: string;
    description?: string;
    version?: string;
    author?: string;
    updateUrl?: string;
}

export interface PluginUiData {
    fileName: string;
    name: string;
    description: string;
    version: string;
    author: string;
    updateUrl?: string;
    content: string;
    enabled: boolean;
}

export interface ThemeUiData {
    fileName: string;
    name: string;
    description: string;
    version: string;
    author: string;
    updateUrl?: string;
    css: string;
    isActive?: boolean;
}

export interface ServerStatusUiData {
    exists: boolean;
    running: boolean;
    serverDir: string;
    serverPath: string;
    currentVersion: string | null;
    skippedVersion: string | null;
    ffmpegInstalled?: boolean;
}

export interface ShaderPresetUiData {
    id: string;
    name: string;
    category: string;
    tier: string;
    description: string;
    installed: boolean;
    fileCount: number;
}

export interface ChapterInfo {
    title: string;
    time: number;
}

export interface PlaybackStats {
    mediaTitle?: string | null;
    path?: string | null;
    videoCodec?: string | null;
    videoFormat?: string | null;
    width?: number | null;
    height?: number | null;
    dwidth?: number | null;
    dheight?: number | null;
    outputWidth?: number | null;
    outputHeight?: number | null;
    containerFps?: number | null;
    estimatedVfFps?: number | null;
    displayFps?: number | null;
    hwdecCurrent?: string | null;
    videoBitrate?: number | null;
    colorPrimaries?: string | null;
    colorGamma?: string | null;
    colorMatrix?: string | null;
    sigPeak?: number | null;
    pixelFormat?: string | null;
    audioCodec?: string | null;
    audioChannels?: string | null;
    audioSampleRate?: number | null;
    audioBitrate?: number | null;
    droppedFrames?: number | null;
    decoderDroppedFrames?: number | null;
    mistimedFrames?: number | null;
    avsync?: number | null;
    cacheDuration?: number | null;
    cacheSpeed?: number | null;
    pausedForCache?: boolean;
    speed?: number | null;
    volume?: number | null;
    timePos?: number | null;
    duration?: number | null;
    activeShaders?: string[];
}

export interface EnhancedData {
    plugins: PluginUiData[];
    themes: ThemeUiData[];
    activeTheme: string | null;
    serverStatus?: ServerStatusUiData;
    clientVersion?: string;
    skippedClientVersion?: string | null;
    thumbnailPreview?: boolean;
    thumbnailPrecache?: boolean;
    thumbnailPreviewSize?: number;
    discordRpc?: boolean;
    shaderPreset?: string;
    shaderPresets?: ShaderPresetUiData[];
    chapterMarkers?: boolean;
}

export interface SettingSchemaItem {
    key: string;
    type?: string;
    title?: string;
    description?: string;
    defaultValue?: any;
    options?: { label: string; value: any }[];
}

export interface MarketplaceMod {
    name: string;
    description: string;
    author: string;
    version?: string;
    preview?: string;
    download: string;
    repo?: string;
}

export interface MarketplaceRegistry {
    plugins: MarketplaceMod[];
    themes: MarketplaceMod[];
}

