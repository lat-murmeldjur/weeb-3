import init, { Sekirei } from "./weeb_3.js";
var sekirei;

self.onconnect = async function (event) {

  await init();

  if (sekirei == undefined){
    console.log('Wings');  
    sekirei = Sekirei.new("");
    sekirei.run("");
  }

  console.log("Clouds")
     
  const port = event.ports[0];

  port.onmessage = function (e) {
    console.log(e.data)
    const workerResult = Sekirei.echo(e.data);
    port.postMessage(workerResult);
  };
};

