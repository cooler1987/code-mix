import 'import_all_needed_packages_here.dart'; // 包含 file_picker, archive 等

// 仿照 Repomix 圖片樣式的深色主題配色
ThemeData darkOrangeTheme = ThemeData(
  brightness: Brightness.dark,
  scaffoldBackgroundColor: const Color(0xFF121316), // 背景極致黑
  primaryColor: const Color(0xFFFF6B00), // Repomix 經典橙色
  cardColor: const Color(0xFF1A1B1F), // 內層卡片深灰
);

// 核心程式碼打包邏輯
void packCodebase({
  required String sourcePath, 
  required String outputPath,
  required String format, // 'XML', 'Markdown', 'Plain'
  required bool includeSummary,
  required bool includeStructure,
}) async {
  final outputBuffer = StringBuffer();
  final dir = Directory(sourcePath);
  
  // 1. 定義預設排除的黑名單 (支援類似 .gitignore 的過濾)
  final ignorePatterns = ['.git', 'node_modules', '.vs', 'build', 'dist', '.png', '.jpg', '.zip'];

  // 2. 遞迴遍歷資料夾
  List<FileSystemEntity> files = dir.listSync(recursive: true);
  List<File> validFiles = [];

  for (var entity in files) {
    if (entity is File) {
      // 簡單的關鍵字過濾邏輯
      bool shouldIgnore = ignorePatterns.any((pattern) => entity.path.contains(pattern));
      if (!shouldIgnore) validFiles.add(entity);
    }
  }

  // 3. 根據勾選框，寫入目錄結構
  if (includeStructure) {
    outputBuffer.writeln("=== DIRECTORY STRUCTURE ===");
    for (var file in validFiles) {
      String relativePath = p.relative(file.path, from: sourcePath);
      outputBuffer.writeln("- $relativePath");
    }
    outputBuffer.writeln("===========================\n");
  }

  // 4. 根據 Output Format 拼接程式碼內容
  for (var file in validFiles) {
    String relativePath = p.relative(file.path, from: sourcePath);
    String content = await file.readAsString(encoding: utf8).catchError((_) => "");

    if (format == 'XML') {
      outputBuffer.writeln('<file path="$relativePath">\n$content\n</file>\n');
    } else if (format == 'Markdown') {
      String ext = p.extension(file.path).replaceAll('.', '');
      outputBuffer.writeln('### File: $relativePath\n```\(ext\n\)content\n```\n');
    } else {
      outputBuffer.writeln('--- START: $relativePath ---\n$content\n--- END ---\n');
    }
  }

  // 5. 輸出最終的文字檔
  final outputFile = File(outputPath);
  await outputFile.writeAsString(outputBuffer.toString());
}
