// The original content is temporarily commented out to allow generating a self-contained demo - feel free to uncomment later.

// import 'package:flutter/material.dart';
// 
// void main() {
//   runApp(const MainApp());
// }
// 
// class MainApp extends StatelessWidget {
//   const MainApp({super.key});
// 
//   @override
//   Widget build(BuildContext context) {
//     return const MaterialApp(
//       home: Scaffold(body: Center(child: Text('Hello World!'))),
//     );
//   }
// }
// 

import 'package:flutter/material.dart';
import 'package:nudge/src/rust/api/simple.dart';
import 'package:nudge/src/rust/frb_generated.dart';

Future<void> main() async {
  await RustLib.init();
  runApp(const MyApp());
}

class MyApp extends StatelessWidget {
  const MyApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      home: Scaffold(
        appBar: AppBar(title: const Text('flutter_rust_bridge quickstart')),
        body: Center(
          child: Text(
            'Action: Call Rust `greet("Tom")`\nResult: `${greet(name: "Tom")}`',
          ),
        ),
      ),
    );
  }
}
