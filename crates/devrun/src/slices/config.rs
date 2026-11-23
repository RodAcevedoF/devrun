use crate::app::App;

pub fn path(app: &App) {
    println!("{}", app.store().path_for(app.root()).display());
}
