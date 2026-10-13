// Which repository documents become site pages. An explicit list on purpose:
// nothing internal can reach the public site through a stray glob. A listed
// file that no longer exists fails the build (scripts/lib/expand.mjs).
export const entries = [
	// Guides
	{ group: 'guides', source: 'waved/docs/WIFI.md', slug: 'guides/wifi', label: 'Wi-Fi & provisioning' },
	{ group: 'guides', source: 'waved/docs/WEBHOOKS.md', slug: 'guides/webhooks', label: 'Recording webhooks' },
	{ group: 'guides', source: 'waved/docs/UPDATES.md', slug: 'guides/updates', label: 'Updates' },

	// Hardware & protocol reference
	{ group: 'reference', source: 'waved/docs/AUDIO_HARDWARE.md', slug: 'reference/audio-hardware', label: 'Audio hardware' },
	{ group: 'reference', source: 'waved/docs/COMPUTE_HARDWARE.md', slug: 'reference/compute-hardware', label: 'Compute hardware' },
	{ group: 'reference', source: 'waved/docs/DISPLAY_HARDWARE.md', slug: 'reference/display-hardware', label: 'Display' },
	{ group: 'reference', source: 'waved/docs/MCU_PROTOCOL.md', slug: 'reference/mcu-protocol', label: 'MCU protocol' },
	{ group: 'reference', source: 'waved/docs/BLUETOOTH_PROTOCOL.md', slug: 'reference/bluetooth-protocol', label: 'Bluetooth protocol' },
	{ group: 'reference', source: 'waved/docs/WAVE_CONTROLLER_PROTOCOL.md', slug: 'reference/wave-controller-protocol', label: 'Wave Controller protocol' },
	{ group: 'reference', source: 'waved/docs/ui/COORDINATE_SYSTEM.md', slug: 'reference/ui-coordinates', label: 'UI coordinate system' },

	// Research
	{ group: 'research', source: 'docs/REPORT.md', slug: 'research/report', label: 'Reverse-engineering report' },
	{ group: 'research', source: 'docs/QUESTIONS_SECURITY.md', slug: 'research/security', label: 'Security Q&A' },
	{ group: 'research', source: 'docs/REPORT_DEVICE_DUMP.md', slug: 'research/device-dump', label: 'Device dump' },
	{ group: 'research', source: 'docs/REPORT_AI_MODELS.md', slug: 'research/ai-models', label: 'AI models' },
	{ group: 'research', source: 'docs/REPORT_NPU.md', slug: 'research/npu-report', label: 'NPU report' },
	{ group: 'research', source: 'docs/REPORT_UI.md', slug: 'research/ui-report', label: 'Stock UI report' },
	{ group: 'research', source: 'docs/FIRMWARE_GAP_ANALYSIS.md', slug: 'research/gap-analysis', label: 'Firmware gap analysis' },
	{ group: 'research', source: 'docs/DEBUG_LATENCY.md', slug: 'research/debug-latency', label: 'Debugging latency' },
	{ group: 'research', source: 'docs/DEBUG_PIPELINE.md', slug: 'research/debug-pipeline', label: 'Debugging the audio pipeline' },
	{ group: 'research-access', source: 'docs/GUIDE_DEVICE_ACCESS.md', slug: 'research/device-access', label: 'Device access' },
	{ group: 'research-access', source: 'docs/GUIDE_EXTRACT_MODELS.md', slug: 'research/extract-models', label: 'Extracting models' },
	{ group: 'research-access', source: 'docs/GUIDE_MODEL_PARTITION_RECOVERY.md', slug: 'research/model-partition-recovery', label: 'Model partition recovery' },
	{ group: 'research-analysis', source: 'docs/analysis_reports/Cloud_Command_Protocol.md', slug: 'research/analysis/cloud-command-protocol', label: 'Cloud command protocol' },
	{ group: 'research-analysis', source: 'docs/analysis_reports/Cloud_Protocol_Decompiled_Analysis.md', slug: 'research/analysis/cloud-protocol', label: 'Cloud protocol' },
	{ group: 'research-analysis', source: 'docs/analysis_reports/Cloud_and_Audio_Library_Analysis.md', slug: 'research/analysis/cloud-and-audio-libraries', label: 'Cloud & audio libraries' },
	{ group: 'research-analysis', source: 'docs/analysis_reports/Init_Scripts_and_Config_Analysis.md', slug: 'research/analysis/init-scripts', label: 'Init scripts & config' },
	{ group: 'research-analysis', source: 'docs/analysis_reports/MQTT_Topic_Map.md', slug: 'research/analysis/mqtt-topics', label: 'MQTT topic map' },
	{ group: 'research-analysis', source: 'docs/analysis_reports/NPU_Stack_Inventory.md', slug: 'research/analysis/npu-stack', label: 'NPU stack inventory' },
	{ group: 'research-analysis', source: 'docs/analysis_reports/UI_and_Database_Decompiled_Analysis.md', slug: 'research/analysis/ui-and-database', label: 'UI & database' },
	{ group: 'research-analysis', source: 'docs/analysis_reports/libins_entry_Binary_Analysis.md', slug: 'research/analysis/libins-entry-binary', label: 'libins_entry: binary' },
	{ group: 'research-analysis', source: 'docs/analysis_reports/libins_entry_Decompiled_Analysis.md', slug: 'research/analysis/libins-entry-decompiled', label: 'libins_entry: decompiled' },
	{ group: 'research-stock-ui', source: 'waved/docs/ui/ASSETS.md', slug: 'research/stock-ui/assets', label: 'Stock UI assets' },
	{ group: 'research-stock-ui', source: 'waved/docs/ui/HOME_SCREEN.md', slug: 'research/stock-ui/home-screen', label: 'Stock home screen' },
	{ group: 'research-npu', glob: 'docs/npu', slugPrefix: 'research/npu', exclude: ['lab-notebook/000-template.md'] },
];

// Screens rendered by the UI code itself (waved-ui snapshot tests), copied to
// src/assets/screens/<name>.png for the landing page and the features tour.
export const screens = [
	'home', 'home_muted', 'dial_live_talker', 'recording', 'recording_hold_stop', 'files', 'file_detail',
	'screen_menu', 'screen_audio_mode', 'screen_bluetooth', 'screen_call_active', 'screen_pin',
	'screen_mic_height', 'screen_uploads', 'update_installing', 'wifi_qr',
];
