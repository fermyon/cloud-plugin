pub trait Interactor {
    fn input(&self, prompt: &str, default_text: &str) -> std::io::Result<String>;
    fn select<T: ToString>(
        &self,
        prompt: &str,
        items: &[T],
        default_index: usize,
    ) -> std::io::Result<Option<usize>>;
}

pub fn interactive() -> impl Interactor {
    Interactive
}

struct Interactive;

impl Interactor for Interactive {
    fn input(&self, prompt: &str, default_text: &str) -> std::io::Result<String> {
        dialoguer::Input::new()
            .with_prompt(prompt)
            .default(default_text.to_string())
            .interact_text()
    }

    fn select<T: ToString>(
        &self,
        prompt: &str,
        items: &[T],
        default_index: usize,
    ) -> std::io::Result<Option<usize>> {
        dialoguer::Select::new()
            .with_prompt(prompt)
            .items(items)
            .default(default_index)
            .interact_opt()
    }
}
