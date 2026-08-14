// file-meta — pure helpers for file type display, quick-preview image
// detection, and EXIF field formatting. No DOM, no globals beyond __t/unquote.

import { __t } from "./i18n.js";
import { unquote } from "./utils.js";

var QUICK_PREVIEW_IMAGE_EXTS = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "tiff", "tif", "heic", "heif", "avif"];

export function isQuickPreviewImage(name) {
  var i = name.lastIndexOf(".");
  if (i === -1) return false;
  return QUICK_PREVIEW_IMAGE_EXTS.indexOf(name.slice(i + 1).toLowerCase()) !== -1;
}

// Map EXIF field names to human-readable labels and format values.
export function getExifFields(data) {
  var labelMap = {
    "Make": __t('exif.make'),
    "Model": __t('exif.model'),
    "DateTimeOriginal": __t('exif.date_taken'),
    "ExposureTime": __t('exif.exposure'),
    "FNumber": __t('exif.aperture'),
    "FocalLength": __t('exif.focal_length'),
    "ISOSpeed": __t('exif.iso'),
    "Flash": __t('exif.flash'),
    "Software": __t('exif.software'),
    "GPSLatitude": __t('exif.gps_latitude'),
    "GPSLongitude": __t('exif.gps_longitude'),
    "PixelXDimension": __t('exif.width'),
    "PixelYDimension": __t('exif.height'),
    "Orientation": __t('exif.orientation')
  };
  // Simple value formatters for certain fields (unquote is from utils)
  var formatters = {
    "ISOSpeed": function (v) { return unquote(v); },
    "ExposureTime": function (v) { return unquote(v); },
    "FNumber": function (v) { return unquote(v).replace(/^F\//, "f/"); },
    "FocalLength": function (v) { return unquote(v); },
    "Flash": function (v) {
      var val = parseInt(v, 10);
      if (isNaN(val)) return v;
      // Bit 0: flash fired
      return (val & 1) ? __t('common.yes') : __t('common.no');
    },
    "PixelXDimension": function (v) { return unquote(v) + " px"; },
    "PixelYDimension": function (v) { return unquote(v) + " px"; },
    "DateTimeOriginal": function (v) { return unquote(v); },
    "Make": function (v) { return unquote(v); },
    "Model": function (v) { return unquote(v); },
    "Software": function (v) { return unquote(v); },
    "GPSLatitude": function (v) { return unquote(v); },
    "GPSLongitude": function (v) { return unquote(v); },
    "Orientation": function (v) {
      var m = {
        "1": __t('exif.orientation_normal'),
        "2": __t('exif.orientation_mirrored'),
        "3": __t('exif.orientation_upside_down'),
        "4": __t('exif.orientation_rotated_180'),
        "5": __t('exif.orientation_mirrored_90_cw'),
        "6": __t('exif.orientation_90_cw'),
        "7": __t('exif.orientation_mirrored_90_ccw'),
        "8": __t('exif.orientation_90_ccw')
      };
      var val = unquote(v);
      return m[val] || v;
    }
  };
  var order = [
    "Make", "Model", "DateTimeOriginal",
    "ExposureTime", "FNumber", "ISOSpeed", "FocalLength", "Flash",
    "Software",
    "GPSLatitude", "GPSLongitude",
    "PixelXDimension", "PixelYDimension",
    "Orientation"
  ];
  var result = [];
  for (var i = 0; i < order.length; i++) {
    var key = order[i];
    var raw = data[key];
    if (raw === undefined || raw === null) continue;
    var label = labelMap[key] || key;
    var value = formatters[key] ? formatters[key](raw) : raw;
    result.push({ label: label, value: value });
  }
  return result;
}

export function humanType(type, ext) {
  if (type === "dir") return __t('ft.folder');
  if (!ext) return __t('ft.file');
  var map = {
    "PNG": __t('ft.png_image'), "JPG": __t('ft.jpeg_image'), "JPEG": __t('ft.jpeg_image'),
    "GIF": __t('ft.gif_image'), "WEBP": __t('ft.webp_image'), "BMP": __t('ft.bmp_image'),
    "SVG": __t('ft.svg_image'),
    "PDF": __t('ft.pdf_document'),
    "DOC": __t('ft.word_document'), "DOCX": __t('ft.word_document'),
    "XLS": __t('ft.excel_spreadsheet'), "XLSX": __t('ft.excel_spreadsheet'),
    "PPT": __t('ft.powerpoint'), "PPTX": __t('ft.powerpoint'),
    "TXT": __t('ft.text_file'), "MD": __t('ft.markdown_file'),
    "RS": __t('ft.rust_source'), "PY": __t('ft.python_script'), "JS": __t('ft.javascript_file'),
    "TS": __t('ft.typescript_file'), "GO": __t('ft.go_source'), "JAVA": __t('ft.java_source'),
    "C": __t('ft.c_source'), "CPP": __t('ft.cpp_source'), "H": __t('ft.header_file'),
    "RB": __t('ft.ruby_script'), "PHP": __t('ft.php_script'), "SH": __t('ft.shell_script'),
    "HTML": __t('ft.html_file'), "CSS": __t('ft.css_file'),
    "TOML": __t('ft.toml_file'), "JSON": __t('ft.json_file'), "YAML": __t('ft.yaml_file'), "YML": __t('ft.yaml_file'),
    "CSV": __t('ft.csv_file'), "XML": __t('ft.xml_file'), "SQL": __t('ft.sql_file'),
    "ZIP": __t('ft.zip_archive'), "TAR": __t('ft.tar_archive'), "GZ": __t('ft.gz_archive'),
    "BZ2": __t('ft.bz2_archive'), "7Z": __t('ft.sevenzip_archive'), "RAR": __t('ft.rar_archive'),
    "MP4": __t('ft.mp4_video'), "MOV": __t('ft.mov_video'), "AVI": __t('ft.avi_video'),
    "MKV": __t('ft.mkv_video'), "WEBM": __t('ft.webm_video'), "WMV": __t('ft.wmv_video'),
    "FLV": __t('ft.flv_video'), "3GP": __t('ft.3gp_video'),
    "MP3": __t('ft.mp3_audio'), "FLAC": __t('ft.flac_audio'), "WAV": __t('ft.wav_audio'),
    "OGG": __t('ft.ogg_audio'), "M4A": __t('ft.m4a_audio'), "AAC": __t('ft.aac_audio'),
    "WMA": __t('ft.wma_audio'), "OPUS": __t('ft.opus_audio'),
    "ISO": __t('ft.disk_image')
  };
  return map[ext] || ext + " File";
}
