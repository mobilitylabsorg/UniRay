import jsQR from "jsqr";
import QRCode from "qrcode";

/** Longest side an image is scaled down to before scanning. */
const MAX_SCAN_SIDE = 1600;

/** Reads the text of a QR code from an image file, `null` when there is none. */
export async function decodeQrImage(file: Blob): Promise<string | null> {
  const bitmap = await createImageBitmap(file);
  const scale = Math.min(1, MAX_SCAN_SIDE / Math.max(bitmap.width, bitmap.height));
  const canvas = document.createElement("canvas");
  canvas.width = Math.round(bitmap.width * scale);
  canvas.height = Math.round(bitmap.height * scale);
  const context = canvas.getContext("2d", { willReadFrequently: true });
  if (!context) return null;
  context.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
  bitmap.close();
  const pixels = context.getImageData(0, 0, canvas.width, canvas.height);
  return jsQR(pixels.data, pixels.width, pixels.height)?.data || null;
}

/** Renders `text` as a QR code, returned as a data URL. */
export function renderQr(text: string): Promise<string> {
  return QRCode.toDataURL(text, {
    errorCorrectionLevel: "M",
    margin: 2,
    width: 560,
    // Dark-on-light regardless of the app theme: scanners expect it.
    color: { dark: "#000000", light: "#ffffff" },
  });
}
