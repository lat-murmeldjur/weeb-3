import init, { Weeb3 } from "./weeb_3.js";
var weeb3;
var workerResult;

self.onconnect = async function (event) {

  await init();

  if (weeb3 == undefined){
    console.log('Wings');  
    weeb3 = Weeb3.new("");
    weeb3.run("");
  }

  console.log("Clouds")
     
  const port = event.ports[0];


  port.onmessage = async function (e) {
    console.log(e.data)
    if (e.data.type0 == "file") {
      var workerResultPromise0 = await weeb3.post_upload(e.data.file0, e.data.encryption0, e.data.index0, e.data.feed0, e.data.topic0);
      port.postMessage(workerResultPromise0);
    } else if (e.data.type0 == "bootnode_settings") {
      var workerResultPromise1 = await weeb3.change_bootnode_address(e.data.bootnode_address0, e.data.network_id0);
      port.postMessage(workerResultPromise1);
    } else if (e.data.type0 == "stamp_reset") {
      var workerResultPromise2 = await weeb3.reset_stamp();
      port.postMessage(workerResultPromise2);
    } else {
      workerResult = await weeb3.acquire(e.data);
      port.postMessage(workerResult);
    }
    workerResult = rn();
  };

};


function rn() {
    return(null);
}

