//! Provedor de recursos para o blitz, restrito a `data:`.
//!
//! REGRA DE SEGURANÇA: o HTML processado é não confiável. Qualquer esquema que
//! não seja `data:` é descartado sem abrir socket ou arquivo — o handler nem
//! chega a ser chamado, então o blitz simplesmente nunca recebe aquele recurso.

use blitz_dom::net::Resource;
use blitz_traits::net::{BoxedHandler, Bytes, NetCallback, NetProvider, Request};
use std::sync::{Arc, Mutex};

/// Coleta os recursos resolvidos para o chamador aplicá-los com
/// `Document::load_resource`. Não há event loop aqui: tudo é síncrono.
#[derive(Default)]
pub struct ColetorDeRecursos {
    recursos: Mutex<Vec<Resource>>,
}

impl ColetorDeRecursos {
    pub fn drenar(&self) -> Vec<Resource> {
        std::mem::take(&mut *self.recursos.lock().unwrap())
    }
}

impl NetCallback<Resource> for ColetorDeRecursos {
    fn call(&self, _doc_id: usize, resultado: Result<Resource, Option<String>>) {
        if let Ok(r) = resultado {
            self.recursos.lock().unwrap().push(r);
        }
    }
}

pub struct ProvedorDataUri {
    coletor: Arc<ColetorDeRecursos>,
}

impl ProvedorDataUri {
    pub fn new(coletor: Arc<ColetorDeRecursos>) -> Self {
        Self { coletor }
    }
}

impl NetProvider<Resource> for ProvedorDataUri {
    fn fetch(&self, doc_id: usize, request: Request, handler: BoxedHandler<Resource>) {
        let Some(bytes) = crate::resource::resolve_data_uri(request.url.as_str()) else {
            return;
        };
        handler.bytes(doc_id, Bytes::from(bytes), self.coletor.clone());
    }
}
