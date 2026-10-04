import { defineConfig } from 'vitest/config';
export default defineConfig({ test: { environmentOptions: { happyDOM: { settings: {
  disableJavaScriptEvaluation: true, disableJavaScriptFileLoading: true,
  disableCSSFileLoading: true, disableIframePageLoading: true,
  enableFileSystemHttpRequests: false,
  navigation: { disableMainFrameNavigation: true, disableChildFrameNavigation: true, disableChildPageNavigation: true },
} } } } });
