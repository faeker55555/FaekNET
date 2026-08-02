use std::io;
fn main() {
    
    let mut numbers = String::new();
    println!("Число:");
    io::stdin().read_line(&mut numbers).expect("Чё то тут не чисто");
    
    let number: i32 = numbers.trim().parse().expect("Число вводи!");










    let mut words = String::new();

    println!("ура?");
    io::stdin().read_line(&mut words).expect("Чё то тут не так");

    let worda = words.trim();
    println!("{number} - {worda}");



    if (30 < number) && (number < 160) {
        println!("ура");
    } else {
        println!("не ура");
    }











}
