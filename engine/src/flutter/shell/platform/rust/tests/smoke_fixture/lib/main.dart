import 'package:flutter/material.dart';

void main() => runApp(const MaterialApp(home: SmokeApp()));

class SmokeApp extends StatefulWidget {
  const SmokeApp({super.key});

  @override
  State<SmokeApp> createState() => _SmokeAppState();
}

class _SmokeAppState extends State<SmokeApp> {
  int count = 0;

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(title: const Text('Flutter Rust Shell Windows smoke')),
    body: Padding(
      padding: const EdgeInsets.all(24),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Text('Count: $count', style: Theme.of(context).textTheme.headlineMedium),
          const SizedBox(height: 16),
          ElevatedButton(onPressed: () => setState(() => count++), child: const Text('Increment')),
          const SizedBox(height: 16),
          const TextField(decoration: InputDecoration(labelText: 'Type here')),
        ],
      ),
    ),
  );
}
