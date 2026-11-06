import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:file_picker/file_picker.dart';
import 'package:path/path.dart' as p;
import 'package:archive/archive.dart';

void main() {
  runApp(const MyApp());
}

class MyApp extends StatelessWidget {
  const MyApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Repomix Flutter Clone',
      debugShowCheckedModeBanner: false,
      theme: ThemeData(
        brightness: Brightness.dark,
        scaffoldBackgroundColor: const Color(0xFF121316),
        primaryColor: const Color(0xFFFF6B00),
        cardColor: const Color(0xFF1A1B1F),
        hintColor: const Color(0xFFA0A0A0),
      ),
      home: const HomeScreen(),
    );
  }
}

class HomeScreen extends StatefulWidget {
  const HomeScreen({super.key});

  @override
  State<HomeScreen> createState() => _HomeScreenState();
}

class _HomeScreenState extends State<HomeScreen> {
  int _selectedInputType = 1; // 0: Git URL, 1: Folder, 2: Zip
  String _selectedPathOrUrl = '';
  String _displayPath = '點擊選擇本地專案資料夾...';
  String _outputFormat = 'Markdown';
  bool _includeSummary = true;
  bool _includeStructure = true;
  String _statusMessage = '就緒。請選擇輸入源開始打包。';
  bool _isProcessing = false;

  @override
  void initState() {
    super.initState();
    _updateDisplayPlaceholder();
  }

  void _updateDisplayPlaceholder() {
    if (_selectedInputType == 0) {
      _displayPath = _selectedPathOrUrl.isEmpty
          ? '請輸入 GitHub 倉庫克隆連結...'
          : _selectedPathOrUrl;
    } else if (_selectedInputType == 1) {
      _displayPath = _selectedPathOrUrl.isEmpty
          ? '點擊選擇本地專案資料夾...'
          : p.basename(_selectedPathOrUrl);
    } else {
      _displayPath = _selectedPathOrUrl.isEmpty
          ? '點擊選擇本地專案 .zip 壓縮檔案...'
          : p.basename(_selectedPathOrUrl);
    }
  }

  Future<void> _handlePathSelection() async {
    if (_selectedInputType == 1) {
      final selectedDirectory =
          await FilePicker.platform.getDirectoryPath(
        dialogTitle: '選擇程式碼專案資料夾',
      );

      if (selectedDirectory != null) {
        setState(() {
          _selectedPathOrUrl = selectedDirectory;
          _updateDisplayPlaceholder();
        });
      }
    } else if (_selectedInputType == 2) {
      final result = await FilePicker.platform.pickFiles(
        type: FileType.custom,
        allowedExtensions: ['zip'],
        dialogTitle: '選擇項目的 ZIP 壓縮包',
      );

      if (result != null && result.files.single.path != null) {
        setState(() {
          _selectedPathOrUrl = result.files.single.path!;
          _updateDisplayPlaceholder();
        });
      }
    } else {
      _showUrlDialog();
    }
  }

  void _showUrlDialog() {
    final urlController = TextEditingController(text: _selectedPathOrUrl);

    showDialog(
      context: context,
      builder: (context) => AppAlertDialog(
        urlController: urlController,
        onConfirm: (value) {
          setState(() {
            _selectedPathOrUrl = value;
            _updateDisplayPlaceholder();
          });
        },
      ),
    );
  }

  Future<void> _startPacking() async {
    if (_selectedPathOrUrl.isEmpty) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('請先選擇或輸入程式碼來源！')),
      );
      return;
    }

    final outputFile = await FilePicker.platform.saveFile(
      dialogTitle: '儲存打包結果',
      fileName: 'codebase_for_llm.txt',
      type: FileType.custom,
      allowedExtensions: ['txt', 'md', 'xml'],
    );

    if (outputFile == null) return;

    setState(() {
      _isProcessing = true;
      _statusMessage = '正在處理打包，請稍候...';
    });

    try {
      String targetDirPath = _selectedPathOrUrl;
      Directory? tempDir;

      if (_selectedInputType == 2) {
        tempDir = Directory(
          p.join(Directory.systemTemp.path, 'repomix_flutter_extracted'),
        );

        if (tempDir.existsSync()) {
          tempDir.deleteSync(recursive: true);
        }
        tempDir.createSync();

        final bytes = File(_selectedPathOrUrl).readAsBytesSync();
        final archive = ZipDecoder().decodeBytes(bytes);

        for (final file in archive) {
          final filename = file.name;

          if (file.isFile) {
            final data = file.content as List<int>;
            final filePath = p.join(tempDir.path, filename);

            File(filePath)
              ..createSync(recursive: true)
              ..writeAsBytesSync(data);
          } else {
            Directory(p.join(tempDir.path, filename)).createSync(recursive: true);
          }
        }

        targetDirPath = tempDir.path;
      } else if (_selectedInputType == 0) {
        tempDir = Directory(
          p.join(Directory.systemTemp.path, 'repomix_flutter_git'),
        );

        if (tempDir.existsSync()) {
          tempDir.deleteSync(recursive: true);
        }

        setState(() => _statusMessage = '正在複製遠端倉庫 (需本地安裝 Git)...');

        final result = await Process.run(
          'git',
          ['clone', '--depth', '1', _selectedPathOrUrl, tempDir.path],
        );

        if (result.exitCode != 0) {
          throw Exception('Git 複製失敗: ${result.stderr}');
        }

        targetDirPath = tempDir.path;
      }

      final finalBuffer = StringBuffer();
      final sourceDir = Directory(targetDirPath);
      const defaultIgnore = [
        '.git',
        'node_modules',
        'build',
        'dist',
        '.png',
        '.jpg',
        '.jpeg',
        '.zip',
        '.exe',
        '.dll',
      ];

      final allEntities = sourceDir.listSync(recursive: true);
      final validFiles = <File>[];

      for (final entity in allEntities) {
        if (entity is File) {
          final relPath = p.relative(entity.path, from: targetDirPath);
          final shouldIgnore = defaultIgnore.any(
            (pattern) =>
                relPath.contains(pattern) ||
                p.basename(relPath).startsWith('.'),
          );

          if (!shouldIgnore) {
            validFiles.add(entity);
          }
        }
      }

      validFiles.sort((a, b) => a.path.compareTo(b.path));

      if (_includeStructure) {
        finalBuffer.writeln('=== DIRECTORY STRUCTURE ===');
        for (final file in validFiles) {
          final relPath = p
              .relative(file.path, from: targetDirPath)
              .replaceAll('\\', '/');
          finalBuffer.writeln('- $relPath');
        }
        finalBuffer.writeln('===========================\n');
      }

      if (_includeSummary) {
        finalBuffer.writeln('=== FILE SUMMARY ===');
        for (final file in validFiles) {
          final relPath = p
              .relative(file.path, from: targetDirPath)
              .replaceAll('\\', '/');
          finalBuffer.writeln('Path: $relPath');
        }
        finalBuffer.writeln('====================\n');
      }

      finalBuffer.writeln('=== CODEBASE CONTENT ===\n');

      for (final file in validFiles) {
        final relPath = p
            .relative(file.path, from: targetDirPath)
            .replaceAll('\\', '/');

        String content = '';
        try {
          content = file.readAsStringSync(encoding: utf8);
        } catch (_) {
          continue;
        }

        if (_outputFormat == 'XML') {
          finalBuffer.writeln('<file path="$relPath">');
          finalBuffer.writeln(content);
          finalBuffer.writeln('</file>\n');
        } else if (_outputFormat == 'Markdown') {
          final extensionName = p.extension(file.path).replaceAll('.', '');
          finalBuffer.writeln('### File: $relPath');
          finalBuffer.writeln('`$extensionName`');
          finalBuffer.writeln(content);
          finalBuffer.writeln();
        } else {
          finalBuffer.writeln('--- START OF FILE: $relPath ---');
          finalBuffer.writeln(content);
          finalBuffer.writeln('--- END OF FILE: $relPath ---\n');
        }
      }

      await File(outputFile).writeAsString(finalBuffer.toString());

      if (tempDir != null && tempDir.existsSync()) {
        tempDir.deleteSync(recursive: true);
      }

      setState(() {
        _statusMessage = '🎉 打包成功！已儲存至: ${p.basename(outputFile)}';
        _isProcessing = false;
      });
    } catch (e) {
      setState(() {
        _statusMessage = '❌ 錯誤: ${e.toString()}';
        _isProcessing = false;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    return Scaffold(
      body: Center(
        child: Container(
          width: 750,
          padding: const EdgeInsets.all(30),
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              Text(
                'Repomix',
                style: TextStyle(
                  color: theme.primaryColor,
                  fontSize: 38,
                  fontWeight: FontWeight.bold,
                  letterSpacing: 1.2,
                ),
              ),
              const SizedBox(height: 5),
              Text(
                'Pack your codebase into AI-friendly formats',
                style: TextStyle(
                  color: theme.hintColor,
                  fontSize: 16,
                ),
              ),
              const SizedBox(height: 35),
              Container(
                decoration: BoxDecoration(
                  color: theme.cardColor,
                  borderRadius: BorderRadius.circular(8),
                  border: Border.all(
                    color: const Color(0xFF2D2F34),
                    width: 1,
                  ),
                ),
                padding: const EdgeInsets.all(20),
                child: Column(
                  children: [
                    Row(
                      children: [
                        ToggleButtons(
                          isSelected: [
                            _selectedInputType == 0,
                            _selectedInputType == 1,
                            _selectedInputType == 2,
                          ],
                          onPressed: (index) {
                            setState(() {
                              _selectedInputType = index;
                              _selectedPathOrUrl = '';
                              _updateDisplayPlaceholder();
                            });
                          },
                          fillColor: theme.primaryColor,
                          selectedColor: Colors.white,
                          color: theme.hintColor,
                          borderRadius: BorderRadius.circular(6),
                          constraints: const BoxConstraints(
                            minWidth: 55,
                            minHeight: 36,
                          ),
                          children: const [
                            Icon(Icons.link, size: 18),
                            Icon(Icons.folder_open, size: 18),
                            Icon(Icons.inventory_2_outlined, size: 18),
                          ],
                        ),
                        const SizedBox(width: 12),
                        Expanded(
                          child: InkWell(
                            onTap: _isProcessing ? null : _handlePathSelection,
                            child: Container(
                              height: 36,
                              padding:
                                  const EdgeInsets.symmetric(horizontal: 12),
                              decoration: BoxDecoration(
                                border: Border.all(
                                  color: const Color(0xFF40434A),
                                  width: 1,
                                ),
                                borderRadius: BorderRadius.circular(6),
                              ),
                              alignment: Alignment.centerLeft,
                              child: Text(
                                _displayPath,
                                style: TextStyle(
                                  color: _selectedPathOrUrl.isEmpty
                                      ? theme.hintColor
                                      : theme.primaryColor,
                                  fontSize: 13,
                                  overflow: TextOverflow.ellipsis,
                                ),
                              ),
                            ),
                          ),
                        ),
                        const SizedBox(width: 12),
                        ElevatedButton.icon(
                          onPressed: _isProcessing ? null : _startPacking,
                          style: ElevatedButton.styleFrom(
                            backgroundColor: theme.primaryColor,
                            foregroundColor: Colors.white,
                            minimumSize: const Size(95, 36),
                            shape: RoundedRectangleBorder(
                              borderRadius: BorderRadius.circular(6),
                            ),
                          ),
                          icon: const Icon(Icons.all_inclusive, size: 16),
                          label: const Text(
                            'Pack',
                            style: TextStyle(fontWeight: FontWeight.bold),
                          ),
                        ),
                        const SizedBox(width: 8),
                        IconButton(
                          onPressed: () {
                            setState(() {
                              _selectedPathOrUrl = '';
                              _updateDisplayPlaceholder();
                              _statusMessage = '狀態已重置。';
                            });
                          },
                          icon: const Icon(Icons.refresh, size: 20),
                          style: IconButton.styleFrom(
                            backgroundColor: const Color(0xFF2D2F34),
                            foregroundColor: Colors.white,
                            shape: RoundedRectangleBorder(
                              borderRadius: BorderRadius.circular(6),
                            ),
                            minimumSize: const Size(36, 36),
                          ),
                        ),
                      ],
                    ),
                    const SizedBox(height: 25),
                    Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              'Output Format',
                              style: TextStyle(
                                color: theme.hintColor,
                                fontSize: 11,
                              ),
                            ),
                            const SizedBox(height: 6),
                            ToggleButtons(
                              isSelected: [
                                _outputFormat == 'XML',
                                _outputFormat == 'Markdown',
                                _outputFormat == 'Plain',
                              ],
                              onPressed: (index) {
                                final formats = ['XML', 'Markdown', 'Plain'];
                                setState(() {
                                  _outputFormat = formats[index];
                                });
                              },
                              fillColor: theme.primaryColor,
                              selectedColor: Colors.white,
                              color: theme.hintColor,
                              borderRadius: BorderRadius.circular(6),
                              constraints: const BoxConstraints(
                                minWidth: 80,
                                minHeight: 32,
                              ),
                              children: const [
                                Text('XML'),
                                Text('Markdown'),
                                Text('Plain'),
                              ],
                            ),
                          ],
                        ),
                        const Spacer(),
                        Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              'Output Format Options',
                              style: TextStyle(
                                color: theme.hintColor,
                                fontSize: 11,
                              ),
                            ),
                            const SizedBox(height: 2),
                            Row(
                              children: [
                                Checkbox(
                                  value: _includeSummary,
                                  activeColor: theme.primaryColor,
                                  onChanged: (value) {
                                    setState(() {
                                      _includeSummary = value ?? true;
                                    });
                                  },
                                ),
                                const Text(
                                  'Include File Summary',
                                  style: TextStyle(fontSize: 13),
                                ),
                              ],
                            ),
                            Row(
                              children: [
                                Checkbox(
                                  value: _includeStructure,
                                  activeColor: theme.primaryColor,
                                  onChanged: (value) {
                                    setState(() {
                                      _includeStructure = value ?? true;
                                    });
                                  },
                                ),
                                const Text(
                                  'Include Directory Structure',
                                  style: TextStyle(fontSize: 13),
                                ),
                              ],
                            ),
                          ],
                        ),
                      ],
                    ),
                  ],
                ),
              ),
              const SizedBox(height: 30),
              Text(
                _statusMessage,
                style: const TextStyle(
                  color: Color(0xFF606060),
                  fontSize: 12,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class AppAlertDialog extends StatelessWidget {
  final TextEditingController urlController;
  final void Function(String) onConfirm;

  const AppAlertDialog({
    super.key,
    required this.urlController,
    required this.onConfirm,
  });

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('輸入 Git 倉庫連結'),
      content: TextField(
        controller: urlController,
        decoration: const InputDecoration(hintText: 'https://github.com'),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('取消'),
        ),
        TextButton(
          onPressed: () {
            onConfirm(urlController.text);
            Navigator.pop(context);
          },
          child: const Text('確定'),
        ),
      ],
    );
  }
}
