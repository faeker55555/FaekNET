use std::io;
fn main() {

    let mut numbers = String::new();
    println!("Число:");
    io::stdin().read_line(&mut numbers).expect("Чё то тут не чисто");
    
    let number: i32 = numbers.trim().parse().expect("Число вводи!");


    let mut znak = String::new();
    println!("Действие:");
    io::stdin().read_line(&mut znak).expect("Чё то тут не чисто");
    let znak = znak.trim();

    let mut numbers2 = String::new();
    println!("Число2:");
    io::stdin().read_line(&mut numbers2).expect("Чё то тут не чисто");
    
    let number2: i32 = numbers2.trim().parse().expect("Число вводи!");

    println!("Результат:");
    
    if znak == "+" {
        println!("{}", number + number2)
    } else if znak == "-" {
        println!("{}", number - number2)
    } else if znak == "*" {
        println!("{}", number * number2)
    } else if znak == "/" {
        println!("{}", number / number2)
    }else {
        println!("Error")
    }
}