// bootstrap — re-expose the small surface that inline template <script> blocks
// and onclick/onerror attributes still reference. Everything else stays
// module-scoped.
import {
  openRightPanel,
  resetRightPanel,
  openMultiSelectPanel,
  thumbFailed,
  openQuickPreview,
  hideQuickPreview,
} from "./right-panel.js";
import {
  triggerUpload,
  triggerFolderUpload,
  onFileSelected,
  onFolderSelected,
  showNewFolderDialog,
  hideNewFolderDialog,
  toggleMinimize,
  closeProgressDialog,
  submitNewFolder,
  onDragOver,
  onDragEnter,
  onDragLeave,
  onDrop,
  pauseUploadItem,
  resumeUploadItem,
  cancelUploadItem,
  retryUploadItem,
  pauseAllUploads,
  resumeAllUploads,
  cancelAllPending,
  retryAllFailed,
} from "./upload.js";
import { copyUploadLinkUrl, copyShareLinkUrl } from "./upload-link-dialog.js";

window.openRightPanel = openRightPanel;
window.resetRightPanel = resetRightPanel;
window.openMultiSelectPanel = openMultiSelectPanel;
window.thumbFailed = thumbFailed;
window.openQuickPreview = openQuickPreview;
window.hideQuickPreview = hideQuickPreview;

window.triggerUpload = triggerUpload;
window.triggerFolderUpload = triggerFolderUpload;
window.onFileSelected = onFileSelected;
window.onFolderSelected = onFolderSelected;
window.showNewFolderDialog = showNewFolderDialog;
window.hideNewFolderDialog = hideNewFolderDialog;
window.toggleMinimize = toggleMinimize;
window.closeProgressDialog = closeProgressDialog;
window.submitNewFolder = submitNewFolder;
window.onDragOver = onDragOver;
window.onDragEnter = onDragEnter;
window.onDragLeave = onDragLeave;
window.onDrop = onDrop;
window.pauseUploadItem = pauseUploadItem;
window.resumeUploadItem = resumeUploadItem;
window.cancelUploadItem = cancelUploadItem;
window.retryUploadItem = retryUploadItem;
window.pauseAllUploads = pauseAllUploads;
window.resumeAllUploads = resumeAllUploads;
window.cancelAllPending = cancelAllPending;
window.retryAllFailed = retryAllFailed;

window.copyUploadLinkUrl = copyUploadLinkUrl;
window.copyShareLinkUrl = copyShareLinkUrl;
