use crossbeam::channel::{self, Receiver, Sender};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::llm::ImageContent;
use crate::tool::{ToolHandler, ToolResult};
use crate::AgentError;

/// Maximum tool calls before cached image is discarded.
const CACHE_MAX_CALLS: u64 = 5;

/// Request to capture a screen window (Rust → Swift)
pub struct CaptureRequest {
    pub id: String,
    /// Native window ID from find_window results.
    pub window_id: Option<u32>,
    /// Crop region (normalized 0.0–1.0).
    pub crop_x: Option<f64>,
    pub crop_y: Option<f64>,
    pub crop_w: Option<f64>,
    pub crop_h: Option<f64>,
    /// If true, run object detection and return bounding boxes instead of image.
    pub detect: Option<bool>,
    /// If true, run OCR on cached image and return text (used by apply_ocr tool).
    pub apply_ocr: Option<bool>,
    /// Space-delimited keywords for window search (used by find_window tool).
    pub search_keywords: Option<String>,
}

/// Result of a screen capture (Swift → Rust)
pub struct CaptureResult {
    pub id: String,
    pub image_base64: String,
    pub metadata_json: String,
}

/// Channel pairs for the capture bridge.
/// All tools share the request channel (Swift polls a single drain),
/// but each has its own result channel so responses are routed correctly.
pub struct CaptureBridge {
    pub request_tx: Sender<CaptureRequest>,
    pub request_rx: Receiver<CaptureRequest>,
    // capture_screen result channel
    pub capture_result_tx: Sender<CaptureResult>,
    pub capture_result_rx: Receiver<CaptureResult>,
    // find_window result channel
    pub find_result_tx: Sender<CaptureResult>,
    pub find_result_rx: Receiver<CaptureResult>,
    // apply_ocr result channel
    pub ocr_result_tx: Sender<CaptureResult>,
    pub ocr_result_rx: Receiver<CaptureResult>,
    // list_windows result channel
    pub list_result_tx: Sender<CaptureResult>,
    pub list_result_rx: Receiver<CaptureResult>,
}

impl CaptureBridge {
    pub fn new() -> Self {
        let (request_tx, request_rx) = channel::unbounded();
        let (capture_result_tx, capture_result_rx) = channel::unbounded();
        let (find_result_tx, find_result_rx) = channel::unbounded();
        let (ocr_result_tx, ocr_result_rx) = channel::unbounded();
        let (list_result_tx, list_result_rx) = channel::unbounded();
        Self {
            request_tx,
            request_rx,
            capture_result_tx,
            capture_result_rx,
            find_result_tx,
            find_result_rx,
            ocr_result_tx,
            ocr_result_rx,
            list_result_tx,
            list_result_rx,
        }
    }
}

/// Metadata about a cached capture (Rust side tracks this for dynamic_state).
struct CacheInfo {
    metadata: String,
}

/// Tool that captures a specific window by name or process, with optional crop/zoom.
pub struct CaptureScreenTool {
    request_tx: Sender<CaptureRequest>,
    result_rx: Receiver<CaptureResult>,
    next_id: AtomicU64,
    cache: Mutex<Option<CacheInfo>>,
    calls_since_capture: AtomicU64,
}

impl CaptureScreenTool {
    pub fn new(request_tx: Sender<CaptureRequest>, result_rx: Receiver<CaptureResult>) -> Self {
        Self {
            request_tx,
            result_rx,
            next_id: AtomicU64::new(1),
            cache: Mutex::new(None),
            calls_since_capture: AtomicU64::new(0),
        }
    }
}

impl ToolHandler for CaptureScreenTool {
    fn name(&self) -> &str {
        "capture_screen"
    }

    fn description(&self) -> &str {
        "Capture a window screenshot by ID (from find_window). Returns the image for vision analysis."
    }

    fn dynamic_state(&self) -> Option<String> {
        let calls = self.calls_since_capture.load(Ordering::Relaxed);
        let guard = self.cache.lock().unwrap();
        if let Some(ref info) = *guard {
            if calls <= CACHE_MAX_CALLS {
                return Some(format!(
                    "Cached image: {}. Use apply_ocr to extract text.",
                    info.metadata,
                ));
            }
        }
        None
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "window_id": {
                    "type": "integer",
                    "description": "Window ID from find_window results"
                },
                "detect": {
                    "type": "boolean",
                    "description": "Return object/text region bounding boxes instead of image. Only use when layout analysis is specifically needed."
                }
            },
            "required": ["window_id"]
        })
    }

    fn call(&self, args: serde_json::Value) -> Result<ToolResult, AgentError> {
        let window_id = args
            .get("window_id")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .ok_or_else(|| AgentError::ParseError("'window_id' is required".to_string()))?;
        let detect = args.get("detect").and_then(|v| v.as_bool());

        // Expire cache if too many calls since last capture
        let calls = self.calls_since_capture.fetch_add(1, Ordering::SeqCst);
        if calls > CACHE_MAX_CALLS {
            *self.cache.lock().unwrap() = None;
        }

        let id = format!("cap_{}", self.next_id.fetch_add(1, Ordering::SeqCst));

        let request = CaptureRequest {
            id: id.clone(),
            window_id: Some(window_id),
            crop_x: None,
            crop_y: None,
            crop_w: None,
            crop_h: None,
            detect,
            apply_ocr: None,
            search_keywords: None,
        };

        self.request_tx.send(request).map_err(|e| {
            AgentError::InternalError(format!("Failed to send capture request: {}", e))
        })?;

        let result = self
            .result_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|e| {
                AgentError::InternalError(format!("Capture timeout or error: {}", e))
            })?;

        // Update cache on successful capture
        if !result.metadata_json.starts_with("Error") {
            *self.cache.lock().unwrap() = Some(CacheInfo {
                metadata: result.metadata_json.clone(),
            });
            self.calls_since_capture.store(0, Ordering::SeqCst);
        }

        if result.image_base64.is_empty() {
            return Ok(ToolResult::text(result.metadata_json));
        }

        Ok(ToolResult::with_images(
            result.metadata_json,
            vec![ImageContent {
                base64: result.image_base64,
                media_type: "image/png".to_string(),
            }],
        ))
    }
}

/// Tool that searches for windows by space-delimited keywords.
pub struct FindWindowTool {
    request_tx: Sender<CaptureRequest>,
    result_rx: Receiver<CaptureResult>,
    next_id: AtomicU64,
}

impl FindWindowTool {
    pub fn new(request_tx: Sender<CaptureRequest>, result_rx: Receiver<CaptureResult>) -> Self {
        Self {
            request_tx,
            result_rx,
            next_id: AtomicU64::new(1),
        }
    }
}

impl ToolHandler for FindWindowTool {
    fn name(&self) -> &str {
        "find_window"
    }

    fn description(&self) -> &str {
        "Search for windows by keywords. Returns matching windows with title, app name, and size. \
         Use this to discover exact window names before calling capture_screen."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "keywords": {
                    "type": "string",
                    "description": "Space-delimited keywords to match against window title and app name (case-insensitive, all keywords must match)"
                }
            },
            "required": ["keywords"]
        })
    }

    fn call(&self, args: serde_json::Value) -> Result<ToolResult, AgentError> {
        let keywords = args
            .get("keywords")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::ParseError("'keywords' parameter is required".to_string()))?;

        if keywords.trim().is_empty() {
            return Err(AgentError::ParseError("'keywords' must not be empty".to_string()));
        }

        let id = format!("find_{}", self.next_id.fetch_add(1, Ordering::SeqCst));

        let request = CaptureRequest {
            id: id.clone(),
            window_id: None,
            crop_x: None,
            crop_y: None,
            crop_w: None,
            crop_h: None,
            detect: None,
            apply_ocr: None,
            search_keywords: Some(keywords.to_string()),
        };

        self.request_tx.send(request).map_err(|e| {
            AgentError::InternalError(format!("Failed to send find_window request: {}", e))
        })?;

        let result = self
            .result_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|e| {
                AgentError::InternalError(format!("find_window timeout or error: {}", e))
            })?;

        Ok(ToolResult::text(result.metadata_json))
    }
}

/// Tool that lists the user's open windows (filtered to real app windows).
/// Shares the request channel; signals "list mode" to Swift with empty
/// `search_keywords`, and routes its result via the `list_` id prefix.
pub struct ListWindowsTool {
    request_tx: Sender<CaptureRequest>,
    result_rx: Receiver<CaptureResult>,
    next_id: AtomicU64,
}

impl ListWindowsTool {
    pub fn new(request_tx: Sender<CaptureRequest>, result_rx: Receiver<CaptureResult>) -> Self {
        Self {
            request_tx,
            result_rx,
            next_id: AtomicU64::new(1),
        }
    }
}

impl ToolHandler for ListWindowsTool {
    fn name(&self) -> &str {
        "list_windows"
    }

    fn description(&self) -> &str {
        "List the user's currently open windows (window id, title, app, size), \
         excluding system UI, tiny widgets, and incognito browser windows. Use this \
         to survey what the user is working on across the desktop before zooming in \
         with capture_screen on a specific window id."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    fn call(&self, _args: serde_json::Value) -> Result<ToolResult, AgentError> {
        let id = format!("list_{}", self.next_id.fetch_add(1, Ordering::SeqCst));

        // Empty search_keywords is the "list all (filtered)" sentinel for Swift.
        let request = CaptureRequest {
            id: id.clone(),
            window_id: None,
            crop_x: None,
            crop_y: None,
            crop_w: None,
            crop_h: None,
            detect: None,
            apply_ocr: None,
            search_keywords: Some(String::new()),
        };

        self.request_tx.send(request).map_err(|e| {
            AgentError::InternalError(format!("Failed to send list_windows request: {}", e))
        })?;

        let result = self
            .result_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|e| {
                AgentError::InternalError(format!("list_windows timeout or error: {}", e))
            })?;

        Ok(ToolResult::text(result.metadata_json))
    }
}

/// Tool that runs OCR on the cached captured image, with optional crop region.
pub struct ApplyOcrTool {
    request_tx: Sender<CaptureRequest>,
    result_rx: Receiver<CaptureResult>,
    next_id: AtomicU64,
}

impl ApplyOcrTool {
    pub fn new(request_tx: Sender<CaptureRequest>, result_rx: Receiver<CaptureResult>) -> Self {
        Self {
            request_tx,
            result_rx,
            next_id: AtomicU64::new(1),
        }
    }
}

impl ToolHandler for ApplyOcrTool {
    fn name(&self) -> &str {
        "apply_ocr"
    }

    fn description(&self) -> &str {
        "Run OCR on the cached captured image. Returns extracted text with bounding boxes. \
         Use crop_x/y/w/h to OCR a specific region (e.g. from detect results)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "crop_x": {
                    "type": "number",
                    "description": "Left edge of region to OCR (0.0–1.0)"
                },
                "crop_y": {
                    "type": "number",
                    "description": "Top edge of region to OCR (0.0–1.0)"
                },
                "crop_w": {
                    "type": "number",
                    "description": "Width of region to OCR (0.0–1.0)"
                },
                "crop_h": {
                    "type": "number",
                    "description": "Height of region to OCR (0.0–1.0)"
                }
            }
        })
    }

    fn call(&self, args: serde_json::Value) -> Result<ToolResult, AgentError> {
        let crop_x = args.get("crop_x").and_then(|v| v.as_f64());
        let crop_y = args.get("crop_y").and_then(|v| v.as_f64());
        let crop_w = args.get("crop_w").and_then(|v| v.as_f64());
        let crop_h = args.get("crop_h").and_then(|v| v.as_f64());

        let id = format!("ocr_{}", self.next_id.fetch_add(1, Ordering::SeqCst));

        let request = CaptureRequest {
            id: id.clone(),
            window_id: None,
            crop_x,
            crop_y,
            crop_w,
            crop_h,
            detect: None,
            apply_ocr: Some(true),
            search_keywords: None,
        };

        self.request_tx.send(request).map_err(|e| {
            AgentError::InternalError(format!("Failed to send apply_ocr request: {}", e))
        })?;

        let result = self
            .result_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|e| {
                AgentError::InternalError(format!("apply_ocr timeout or error: {}", e))
            })?;

        Ok(ToolResult::text(result.metadata_json))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: spawn a thread that responds to capture requests
    fn mock_swift_side(
        request_rx: Receiver<CaptureRequest>,
        result_tx: Sender<CaptureResult>,
        image: &str,
        metadata: &str,
    ) {
        let image = image.to_string();
        let metadata = metadata.to_string();
        std::thread::spawn(move || {
            let req = request_rx.recv().unwrap();
            result_tx
                .send(CaptureResult {
                    id: req.id,
                    image_base64: image,
                    metadata_json: metadata,
                })
                .unwrap();
        });
    }

    // ---- CaptureScreenTool tests ----

    #[test]
    fn test_capture_by_window_id() {
        let bridge = CaptureBridge::new();
        let tool = CaptureScreenTool::new(bridge.request_tx.clone(), bridge.capture_result_rx.clone());

        let rx = bridge.request_rx.clone();
        let tx = bridge.capture_result_tx.clone();
        std::thread::spawn(move || {
            let req = rx.recv().unwrap();
            assert_eq!(req.window_id, Some(12345));
            tx.send(CaptureResult {
                id: req.id,
                image_base64: "iVBORw0KGgo=".to_string(),
                metadata_json: "Window: Terminal, Size: 800x600".to_string(),
            }).unwrap();
        });

        let result = tool.call(serde_json::json!({"window_id": 12345})).unwrap();
        assert!(result.text.contains("Terminal"));
        assert_eq!(result.images.len(), 1);
    }

    #[test]
    fn test_capture_missing_window_id() {
        let bridge = CaptureBridge::new();
        let tool = CaptureScreenTool::new(bridge.request_tx.clone(), bridge.capture_result_rx.clone());

        let err = tool.call(serde_json::json!({})).unwrap_err();
        assert!(err.to_string().contains("window_id"));
    }

    #[test]
    fn test_capture_error_result() {
        let bridge = CaptureBridge::new();
        let tool = CaptureScreenTool::new(bridge.request_tx.clone(), bridge.capture_result_rx.clone());

        mock_swift_side(
            bridge.request_rx.clone(),
            bridge.capture_result_tx.clone(),
            "",
            "Error: window not found",
        );

        let result = tool.call(serde_json::json!({"window_id": 99999})).unwrap();
        assert!(result.text.contains("Error"));
        assert!(result.images.is_empty());
    }

    #[test]
    fn test_capture_with_detect() {
        let bridge = CaptureBridge::new();
        let tool = CaptureScreenTool::new(bridge.request_tx.clone(), bridge.capture_result_rx.clone());

        let rx = bridge.request_rx.clone();
        let tx = bridge.capture_result_tx.clone();
        std::thread::spawn(move || {
            let req = rx.recv().unwrap();
            assert_eq!(req.window_id, Some(100));
            assert_eq!(req.detect, Some(true));
            tx.send(CaptureResult {
                id: req.id,
                image_base64: String::new(),
                metadata_json: "Object Detection (2 objects):\n  rectangles (2):".to_string(),
            }).unwrap();
        });

        let result = tool.call(serde_json::json!({"window_id": 100, "detect": true})).unwrap();
        assert!(result.text.contains("Object Detection"));
        assert!(result.images.is_empty());
    }

    #[test]
    fn test_dynamic_state_with_cache() {
        let bridge = CaptureBridge::new();
        let tool = CaptureScreenTool::new(bridge.request_tx.clone(), bridge.capture_result_rx.clone());

        assert!(tool.dynamic_state().is_none());

        mock_swift_side(
            bridge.request_rx.clone(),
            bridge.capture_result_tx.clone(),
            "IMG",
            "Window: Chrome, Size: 1920x1080",
        );
        tool.call(serde_json::json!({"window_id": 42})).unwrap();

        let state = tool.dynamic_state().unwrap();
        assert!(state.contains("Cached image"));
        assert!(state.contains("Chrome"));

        // full_description combines static + dynamic
        let full = crate::tool::full_description(&tool);
        assert!(full.starts_with("Capture a window screenshot"));
        assert!(full.contains("[Cached image"));
    }

    // ---- FindWindowTool tests ----

    #[test]
    fn test_find_window_round_trip() {
        let bridge = CaptureBridge::new();
        let tool = FindWindowTool::new(bridge.request_tx.clone(), bridge.find_result_rx.clone());

        let rx = bridge.request_rx.clone();
        let tx = bridge.find_result_tx.clone();
        std::thread::spawn(move || {
            let req = rx.recv().unwrap();
            assert!(req.id.starts_with("find_"));
            assert_eq!(req.search_keywords, Some("chrome".to_string()));
            tx.send(CaptureResult {
                id: req.id,
                image_base64: String::new(),
                metadata_json: "Found 1 window(s):\n  id: 12345 | \"Google\" | app: Chrome | 1920x1080".to_string(),
            }).unwrap();
        });

        let result = tool.call(serde_json::json!({"keywords": "chrome"})).unwrap();
        assert!(result.text.contains("Found 1"));
        assert!(result.text.contains("12345"));
    }

    #[test]
    fn test_find_window_missing_keywords() {
        let bridge = CaptureBridge::new();
        let tool = FindWindowTool::new(bridge.request_tx.clone(), bridge.find_result_rx.clone());

        let err = tool.call(serde_json::json!({})).unwrap_err();
        assert!(err.to_string().contains("keywords"));
    }

    #[test]
    fn test_find_window_empty_keywords() {
        let bridge = CaptureBridge::new();
        let tool = FindWindowTool::new(bridge.request_tx.clone(), bridge.find_result_rx.clone());

        let err = tool.call(serde_json::json!({"keywords": "  "})).unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    // ---- ListWindowsTool tests ----

    #[test]
    fn test_list_windows_round_trip() {
        let bridge = CaptureBridge::new();
        let tool = ListWindowsTool::new(bridge.request_tx.clone(), bridge.list_result_rx.clone());

        let rx = bridge.request_rx.clone();
        let tx = bridge.list_result_tx.clone();
        std::thread::spawn(move || {
            let req = rx.recv().unwrap();
            // list mode: list_ id prefix and the empty-keyword sentinel.
            assert!(req.id.starts_with("list_"));
            assert_eq!(req.search_keywords.as_deref(), Some(""));
            assert!(req.window_id.is_none());
            tx.send(CaptureResult {
                id: req.id,
                image_base64: String::new(),
                metadata_json: "Open windows (2):\n  id: 1 | \"VS Code\" | app: Code | 1440x900".to_string(),
            }).unwrap();
        });

        let result = tool.call(serde_json::json!({})).unwrap();
        assert!(result.text.contains("Open windows (2)"));
        assert!(result.images.is_empty());
    }

    // ---- ApplyOcrTool tests ----

    #[test]
    fn test_apply_ocr_round_trip() {
        let bridge = CaptureBridge::new();
        let tool = ApplyOcrTool::new(bridge.request_tx.clone(), bridge.ocr_result_rx.clone());

        let rx = bridge.request_rx.clone();
        let tx = bridge.ocr_result_tx.clone();
        std::thread::spawn(move || {
            let req = rx.recv().unwrap();
            assert!(req.id.starts_with("ocr_"));
            assert_eq!(req.apply_ocr, Some(true));
            assert!(req.window_id.is_none());
            tx.send(CaptureResult {
                id: req.id,
                image_base64: String::new(),
                metadata_json: "OCR Results (2 entries):\n  [0.1,0.05] \"Hello\" (98%)".to_string(),
            }).unwrap();
        });

        let result = tool.call(serde_json::json!({})).unwrap();
        assert!(result.text.contains("OCR Results"));
        assert!(result.images.is_empty());
    }

    #[test]
    fn test_apply_ocr_with_crop() {
        let bridge = CaptureBridge::new();
        let tool = ApplyOcrTool::new(bridge.request_tx.clone(), bridge.ocr_result_rx.clone());

        let rx = bridge.request_rx.clone();
        let tx = bridge.ocr_result_tx.clone();
        std::thread::spawn(move || {
            let req = rx.recv().unwrap();
            assert_eq!(req.crop_x, Some(0.1));
            assert_eq!(req.crop_y, Some(0.2));
            assert_eq!(req.crop_w, Some(0.5));
            assert_eq!(req.crop_h, Some(0.3));
            tx.send(CaptureResult {
                id: req.id,
                image_base64: String::new(),
                metadata_json: "OCR: cropped text".to_string(),
            }).unwrap();
        });

        let result = tool.call(serde_json::json!({
            "crop_x": 0.1, "crop_y": 0.2, "crop_w": 0.5, "crop_h": 0.3
        })).unwrap();
        assert!(result.text.contains("OCR"));
    }
}
