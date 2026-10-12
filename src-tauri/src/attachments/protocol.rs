//! Protocole `chatonlm://` : images des pièces jointes pour la page
//! (vignettes du composer, images de l'historique), sans passer leurs octets
//! par l'IPC en base64.
//!
//! - `chatonlm://localhost/attachment/<id>` : pièce jointe enregistrée ;
//! - `chatonlm://localhost/staged/<id>` : fichier de la zone de préparation
//!   (`Cache-Control: no-store` : il peut disparaître à tout moment).
//!
//! Images seulement ; tout le reste (texte, inconnu, adresse invalide) : 404.

use tauri::http::{Response, StatusCode, header};

use super::Staging;
use crate::store::Store;
use crate::store::chats::{self, AttachmentKind};

/// Nom du protocole.
pub const SCHEME: &str = "chatonlm";

/// Ressource demandée.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resource {
    Attachment(i64),
    Staged(String),
}

/// Image à renvoyer.
pub struct Image {
    pub mime: String,
    pub data: Vec<u8>,
}

/// Ressource désignée par une adresse (`/attachment/<id>` ou
/// `/staged/<id>`, rien d'autre ; la requête éventuelle est ignorée).
pub fn resource(uri: &str) -> Option<Resource> {
    let path = uri.strip_prefix("chatonlm://")?;
    let path = &path[path.find('/')?..];
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let mut parts = path.strip_prefix('/')?.split('/');
    let resource = match (parts.next()?, parts.next()?) {
        ("attachment", id) if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) => {
            Resource::Attachment(id.parse().ok()?)
        }
        ("staged", id) if uuid::Uuid::parse_str(id).is_ok() => Resource::Staged(id.to_owned()),
        _ => return None,
    };
    parts.next().is_none().then_some(resource)
}

/// Réponse à une adresse : l'image trouvée par `find`, sinon 404.
pub fn respond(uri: &str, find: impl FnOnce(&Resource) -> Option<Image>) -> Response<Vec<u8>> {
    let Some(resource) = resource(uri) else {
        return not_found();
    };
    let Some(image) = find(&resource) else {
        return not_found();
    };
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, image.mime)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    if matches!(resource, Resource::Staged(_)) {
        response = response.header(header::CACHE_CONTROL, "no-store");
    }
    response.body(image.data).unwrap_or_else(|_| not_found())
}

/// Réponse à une adresse : image enregistrée (base absente au démarrage :
/// aucune) ou fichier préparé, images seulement.
pub async fn handle(store: Option<&Store>, staging: &Staging, uri: &str) -> Response<Vec<u8>> {
    let image = match resource(uri) {
        Some(Resource::Staged(id)) => staging.get(&id).and_then(|staged| {
            (staged.file.kind == AttachmentKind::Image).then(|| Image {
                mime: staged.file.mime.clone(),
                data: staged.data.clone(),
            })
        }),
        Some(Resource::Attachment(id)) => match store {
            Some(store) => store
                .call(move |c| chats::image_attachment(c, id))
                .await
                .ok()
                .flatten()
                .map(|(mime, data)| Image { mime, data }),
            None => None,
        },
        None => None,
    };
    respond(uri, |_| image)
}

fn not_found() -> Response<Vec<u8>> {
    let mut response = Response::new(Vec::new());
    *response.status_mut() = StatusCode::NOT_FOUND;
    response
}
