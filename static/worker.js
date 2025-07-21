import init, { Sekirei } from "./weeb_3.js";
var sekirei;
var workerResult;

self.onconnect = async function (event) {

  await init();

  if (sekirei == undefined){
    console.log('Wings');  
    sekirei = Sekirei.new("");
    sekirei.run("");
  }

  console.log("Clouds")
     
  const port = event.ports[0];


  port.onmessage = async function (e) {
    console.log(e.data)
    if (e.data.type0 == "file") {
      var workerResultPromise0 = await sekirei.post_upload(e.data.file0, e.data.encryption0, e.data.index0, e.data.feed0, e.data.topic0);
      port.postMessage(workerResultPromise0);
    } else if (e.data.type0 == "bootnode_settings") {
      var workerResultPromise1 = await sekirei.change_bootnode_address(e.data.bootnode_address0, e.data.network_id0);
      port.postMessage(workerResultPromise1);
    } else if (e.data.type0 == "stamp_reset") {
      var workerResultPromise2 = await sekirei.reset_stamp();
      port.postMessage(workerResultPromise2);
    } else {
      workerResult = await sekirei.acquire(e.data);
      port.postMessage(workerResult);
    }
    workerResult = rn();
  };

};


function rn() {
    return(null);
}

