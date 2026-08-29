package;

class Code_One {
   
	static public function smax_init() {
		var w1:String = './hax/chronicl.dt';
		var w2:String = './hax/featuring.dt';
		var w3:String = './hax/ohio.note';

		clientele('rustup', ['update']);
		clientele('cargo', ['update']);
		clientele('cargo', ['fix', '--allow-staged', '--allow-dirty']);

		Sys.putEnv("RUSTFLAGS", "--cfg getrandom_backend=\"wasm_js\"");
		var wasmBuilt = clientele('wasm-pack', [ '-v', 'build', '--target', 'web', '--out-dir', 'static', '--out-name', 'weeb_3']);
		if (wasmBuilt) {
			sys.io.File.saveContent('./static/.gitignore', '*\n!worker.js\n');
		}
		Sys.putEnv("RUSTFLAGS", null);
		var nativeBuilt = clientele('cargo', ['build']);

		if (nativeBuilt && wasmBuilt) {
			if (!clientele('rm', [ '-rf', './docs/snippets' ])) {
				trace('Warning/Error: Cannot replace generated docs snippets');
				return;
			}
			for (asset in [
				'example.html', 'hls-stream-example.html', 'issue-1-json-sync-example.html',
				'index.html', '404.html', 'weeb_3.js', 'weeb_3_bg.wasm', 'service.js', 'worker.js'
			]) clientele('cp', [ './static/$asset', './docs/' ]);
			clientele('mkdir', [ '-p', './docs/snippets' ]);
			clientele('cp', [ '-R', './static/snippets/.', './docs/snippets/' ]);

			var mist = gitcoal(w1);
			var dome = gitcoal(w2);
			temporas(w3);

			clientele('git', ['checkout', '-b', 'feature-$dome']);
			clientele('git', ['add', '.']);
			clientele('git', ['commit', '-m', 'Commit number $mist']);
			clientele('git', ['push', 'origin', 'feature-$dome']);
			clientele('git', ['checkout', 'main']);
			if (clientele('git', ['merge', 'feature-$dome'])) {
				clientele('git', ['push', 'origin', 'main']);
			}
		}
	}

	static public function clientele(crx:String, ?arx:Array<String>):Bool {
		if (arx == null) arx = [];
		trace('Executing: $crx ${arx.join(" ")}');

		try {
			var exit = Sys.command(crx, arx);
			if (exit == 0) return true;
			trace('Warning/Error: Cannot execute $crx: exited with code $exit');
		} catch (e:Dynamic) {
			trace('Warning/Error: Cannot start $crx: ' + Std.string(e));
		}
		return false;
	}

	static public function temporas(?oh:String) {
		var fame = DateTools.format(Date.now(), "Year::%Y::|::Month::%m::|::Day::%d::|::Hour::%H::|::Minute::%M::|::Second::%S::");
		trace('Current::'+fame);
		if ( oh != null ) { 
			if (!sys.FileSystem.exists(oh) ) {
				sys.io.File.saveContent(oh, '');
			}
			if ( sys.FileSystem.exists(oh) ) {
				var output = sys.io.File.append(oh, false);
				  output.writeString(fame+'\n');
				  output.close();
			}
		}
	} 

	static public function gitcoal(jxmd:String) {
		if (!sys.FileSystem.exists(jxmd)) {
			sys.io.File.saveContent(jxmd, '0');
		}
		var kxmd = sys.io.File.getContent(jxmd); 
		var chr0n = Std.parseInt(kxmd);
		if (kxmd != '') {
			chr0n++;
			kxmd = Std.string(chr0n);
			sys.io.File.saveContent(jxmd, kxmd);
		}
		return chr0n;
	} 

}
