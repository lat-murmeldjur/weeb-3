package;
import haxe.Int64;

class Code_One {
   
	static public function smax_init() {
		var count:Int64 = 0;

		var w1:String = './hax/chronicl.dt';
		var w2:String = './hax/featuring.dt';
		var w3:String = './hax/ohio.note';

		clientele('rustup', ['update'], count);
		clientele('cargo', ['update'], count);
		clientele('cargo', ['fix', '--allow-staged', '--allow-dirty'], count);
		var wbuild:Array<Null<Bool>> = [null];
		var build:Array<Null<Bool>> = [null];
		clientele('cargo', ['build'], count, build);

		Sys.putEnv("RUSTFLAGS", "--cfg getrandom_backend=\"wasm_js\"");
		clientele('wasm-pack', [ '-v', 'build', '--target', 'web', '--out-dir', 'static'], count, wbuild);
		Sys.putEnv("RUSTFLAGS", null);

		if ( build[0] && wbuild[0]) {
			clientele('cp', [ './static/example.html', './docs/' ], count);
			clientele('cp', [ './static/index.html', './docs/' ], count);
			clientele('cp', [ './static/404.html', './docs/' ], count);
			clientele('cp', [ './static/weeb_3.js', './docs/' ], count);
			clientele('cp', [ './static/weeb_3_bg.wasm', './docs/' ], count);
			clientele('cp', [ './static/service.js', './docs/' ], count);

			clientele('mkdir', [ '-p', './docs/snippets/web3-0742d85b024bb6f5' ], count);
			clientele('cp', [ './static/snippets/web3-0742d85b024bb6f5/inline0.js', './docs/snippets/web3-0742d85b024bb6f5/inline0.js' ], count);

			var mist = gitcoal(w1);
			var dome = gitcoal(w2);
			temporas(w3);

			clientele('git', ['checkout', '-b', 'feature-$dome'], count);
			clientele('git', ['add', '.'], count);
			clientele('git', ['commit', '-am', '"Commit number $mist"'], count);
			clientele('git', ['push', 'origin', 'feature-$dome'], count);
			clientele('git', ['checkout', 'main'], count);
			var merge:Array<Null<Bool>> = [null];
			clientele('git', ['merge', 'feature-$dome'], count, merge);
			if ( merge[0] ) {
				clientele('git', ['push', 'origin', 'main'], count);
			}
		}
	}

	static public function clientele(crx:String, ?arx:Array<String>, ?count:Int64, ?really:Array<Null<Bool>>):String {
		trace('Executing: $crx $arx');
		var process_4 = new sys.io.Process('$crx', arx);
		var bounce = process_4.stdout.readAll().toString();
		trace("Announcement: " + bounce);
		if (process_4.exitCode() != 0) {
			var message = process_4.stderr.readAll().toString();
			var pos = haxe.macro.Context.currentPos();
			haxe.macro.Context.warning('Warning/Error: Cannot execute process_$count ... $process_4' + message, pos);
			if (really != null ) {
				really[0] = false;	
			}
			count++;
			return bounce;
		};
		if (really != null ) {
			really[0] = true;	
		}
		count++;
		return bounce;
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